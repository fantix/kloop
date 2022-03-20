# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.

import asyncio.events
import asyncio.futures
import asyncio.trsock
import asyncio.transports
import contextvars
import socket
import ssl

from . import uring, ktls


class Callback:
    __slots__ = ("_callback", "_context", "_args", "_kwargs")

    def __init__(self, callback, context=None, args=None, kwargs=None):
        if context is None:
            context = contextvars.copy_context()
        self._callback = callback
        self._context = context
        self._args = args or ()
        self._kwargs = kwargs or {}

    def __call__(self):
        self._context.run(self._callback, *self._args, **self._kwargs)

    def __repr__(self):
        return f"{self._callback} {self._args} {self._kwargs} {self._context}"


class KLoopSocketTransport(
    asyncio.transports._FlowControlMixin, asyncio.Transport
):
    __slots__ = (
        "_waiter",
        "_sock",
        "_protocol",
        "_closing",
        "_recv_buffer",
        "_recv_buffer_factory",
        "_read_ready_cb",
        "_buffers",
        "_buffer_size",
        "_current_work",
        "_write_waiter",
        "_read_paused",
    )

    def __init__(
        self, loop, sock, protocol, waiter=None, extra=None, server=None
    ):
        super().__init__(extra, loop)
        self._extra["socket"] = asyncio.trsock.TransportSocket(sock)
        try:
            self._extra["sockname"] = sock.getsockname()
        except OSError:
            self._extra["sockname"] = None
        if "peername" not in self._extra:
            try:
                self._extra["peername"] = sock.getpeername()
            except socket.error:
                self._extra["peername"] = None

        self._buffers = []
        self._buffer_size = 0
        self._current_work = None
        self._sock = sock
        self._closing = False
        self._write_waiter = None
        self._read_paused = False

        self.set_protocol(protocol)
        self._waiter = waiter

        self._loop.call_soon(self._protocol.connection_made, self)
        self._loop.call_soon(self._read)

        if self._waiter is not None:
            self._loop.call_soon(
                asyncio.futures._set_result_unless_cancelled,
                self._waiter,
                None,
            )

    def set_protocol(self, protocol):
        if isinstance(protocol, asyncio.BufferedProtocol):
            self._read_ready_cb = self._read_ready__buffer_updated
            self._recv_buffer = None
            self._recv_buffer_factory = protocol.get_buffer
        else:
            self._read_ready_cb = self._read_ready__data_received
            self._recv_buffer = bytearray(256 * 1024)
            self._recv_buffer_factory = lambda _hint: self._recv_buffer
        self._protocol = protocol

    def _read(self):
        self._loop._selector.submit(
            uring.RecvMsgWork(
                self._sock.fileno(),
                [self._recv_buffer_factory(-1)],
                self._read_ready_cb,
            )
        )

    def _read_ready__buffer_updated(self, res):
        if res < 0:
            raise IOError
        elif res == 0:
            self._protocol.eof_received()
        else:
            try:
                # print(f"buffer updated: {res}")
                self._protocol.buffer_updated(res)
            finally:
                if not self._closing:
                    self._read()

    def _read_ready__data_received(self, res):
        if res < 0:
            raise IOError(f"{res}")
        elif res == 0:
            self._protocol.eof_received()
        else:
            try:
                # print(f"data received: {res}")
                self._protocol.data_received(self._recv_buffer[:res])
            finally:
                if not self._closing:
                    self._read()

    def _write_done(self, res):
        self._current_work = None
        if res < 0:
            # TODO: force close transport
            raise IOError()
        self._buffer_size -= res
        if self._buffers:
            if len(self._buffers) == 1:
                self._current_work = uring.SendWork(
                    self._sock.fileno(), self._buffers[0], self._write_done
                )
            else:
                self._current_work = uring.SendMsgWork(
                    self._sock.fileno(), self._buffers, self._write_done
                )
            self._loop._selector.submit(self._current_work)
            self._buffers = []
        elif self._closing:
            self._loop.call_soon(self._call_connection_lost, None)
        elif self._write_waiter is not None:
            self._write_waiter()
            self._write_waiter = None
        self._maybe_resume_protocol()

    def write(self, data):
        self._buffer_size += len(data)
        if self._current_work is None:
            self._current_work = uring.SendWork(
                self._sock.fileno(), data, self._write_done
            )
            self._loop._selector.submit(self._current_work)
        else:
            self._buffers.append(data)
        self._maybe_pause_protocol()

    def close(self):
        if self._closing:
            return
        self._closing = True
        if self._current_work is None:
            self._loop.call_soon(self._call_connection_lost, None)

    def _call_connection_lost(self, exc):
        try:
            if self._protocol is not None:
                self._protocol.connection_lost(exc)
        finally:
            self._sock.close()
            self._sock = None
            self._protocol = None
            self._loop = None

    def get_write_buffer_size(self):
        return self._buffer_size

    def pause_reading(self):
        self._read_paused = True

    def resume_reading(self):
        if self._read_paused:
            self._read_paused = False
            self._read()


class KLoopSSLHandshakeProtocol(asyncio.Protocol):
    __slots__ = (
        "_incoming",
        "_outgoing",
        "_handshaking",
        "_secrets",
        "_app_protocol",
        "_transport",
        "_sslobj",
    )

    def __init__(self, sslcontext, server_hostname):
        self._handshaking = True
        self._secrets = {}
        self._incoming = ssl.MemoryBIO()
        self._outgoing = ssl.MemoryBIO()
        self._sslobj = sslcontext.wrap_bio(
            self._incoming,
            self._outgoing,
            server_side=False,
            server_hostname=server_hostname,
        )

    def connection_made(self, transport):
        self._transport = transport
        self._handshake()

    def data_received(self, data):
        self._incoming.write(data)
        self._handshake()

    def _handshake(self):
        success, secrets = ktls.do_handshake_capturing_secrets(self._sslobj)
        self._secrets.update(secrets)
        if success:
            if self._handshaking:
                self._handshaking = False
                if data := self._outgoing.read():
                    self._transport.write(data)
                    self._transport._write_waiter = self._after_last_write
                    self._transport.pause_reading()
                else:
                    self._after_last_write()
            # else:
            #     try:
            #         data = self._sslobj.read(16384)
            #     except ssl.SSLWantReadError:
            #         data = None
            #     self._transport._upgrade_ktls_read(
            #         self._sslobj,
            #         self._secrets["SERVER_TRAFFIC_SECRET_0"],
            #         data,
            #     )
        else:
            if data := self._outgoing.read():
                self._transport.write(data)

    def _after_last_write(self):
        try:
            data = self._sslobj.read(16384)
        except ssl.SSLWantReadError:
            data = None
        self._transport._upgrade_ktls_write(
            self._sslobj,
            self._secrets["CLIENT_TRAFFIC_SECRET_0"],
        )
        self._transport._upgrade_ktls_read(
            self._sslobj,
            self._secrets["SERVER_TRAFFIC_SECRET_0"],
            data,
        )
        self._transport.resume_reading()


class KLoopSSLTransport(KLoopSocketTransport):
    __slots__ = ("_app_protocol",)

    def __init__(
        self,
        loop,
        sock,
        protocol,
        waiter=None,
        extra=None,
        server=None,
        *,
        sslcontext,
        server_hostname,
    ):
        ktls.enable_ulp(sock)
        self._app_protocol = protocol
        super().__init__(
            loop,
            sock,
            KLoopSSLHandshakeProtocol(sslcontext, server_hostname),
            None,
            extra,
            server,
        )
        self._waiter = waiter

    def _upgrade_ktls_write(self, sslobj, secret):
        ktls.upgrade_aes_gcm_256(sslobj, self._sock, secret, True)
        self._loop.call_soon(self._app_protocol.connection_made, self)
        if self._waiter is not None:
            self._loop.call_soon(
                asyncio.futures._set_result_unless_cancelled,
                self._waiter,
                None,
            )

    def _upgrade_ktls_read(self, sslobj, secret, data):
        ktls.upgrade_aes_gcm_256(sslobj, self._sock, secret, False)
        self.set_protocol(self._app_protocol)
        if data is not None:
            if data:
                self._app_protocol.data_received(data)
            else:
                self._app_protocol.eof_received()


class KLoop(asyncio.BaseEventLoop):
    def __init__(self, args):
        super().__init__()
        self._selector = uring.Ring(*args)

    def _process_events(self, works):
        for work in works:
            work.complete()

    async def sock_connect(self, sock, address):
        fut = self.create_future()
        self._selector.submit(uring.ConnectWork(sock.fileno(), address, fut))
        return await fut

    async def getaddrinfo(
        self, host, port, *, family=0, type=0, proto=0, flags=0
    ):
        return socket.getaddrinfo(host, port, family, type, proto, flags)

    def _make_socket_transport(
        self, sock, protocol, waiter=None, *, extra=None, server=None
    ):
        return KLoopSocketTransport(
            self, sock, protocol, waiter, extra, server
        )

    def _make_ssl_transport(
        self,
        rawsock,
        protocol,
        sslcontext,
        waiter=None,
        *,
        server_side=False,
        server_hostname=None,
        extra=None,
        server=None,
        ssl_handshake_timeout=None,
        call_connection_made=True,
    ):
        if sslcontext is None:
            sslcontext = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
        return KLoopSSLTransport(
            self,
            rawsock,
            protocol,
            waiter,
            extra,
            server,
            sslcontext=sslcontext,
            server_hostname=server_hostname,
        )


class KLoopPolicy(asyncio.events.BaseDefaultEventLoopPolicy):
    __slots__ = ("_selector_args",)

    def __init__(
        self, queue_depth=128, sq_thread_idle=2000, sq_thread_cpu=None
    ):
        super().__init__()
        assert queue_depth in {
            1,
            2,
            4,
            8,
            16,
            32,
            64,
            128,
            256,
            512,
            1024,
            2048,
            4096,
        }
        self._selector_args = (queue_depth, sq_thread_idle, sq_thread_cpu)

    def _loop_factory(self):
        return KLoop(self._selector_args)

    # Child processes handling (Unix only).

    def get_child_watcher(self):
        raise NotImplementedError

    def set_child_watcher(self, watcher):
        raise NotImplementedError
