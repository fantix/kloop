# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


async def tcp_connect(KLoopImpl loop, host, port):
    cdef:
        Resolve resolve
        TCPConnect connector
        int fd, res
        libc.sockaddr * addr
        Handle handle
        size_t i

    resolve = await loop.resolver.lookup_ip(host, port)
    if not resolve.r.result_len:
        raise RuntimeError(f"Cannot resolve host: {host!r}")

    connector.loop = &loop.loop
    connector.ring_cb.callback = tcp_connect_cb
    connector.ring_cb.data = &connector

    exceptions = []
    for i in range(resolve.r.result_len):
        addr = resolve.r.result + i
        fd = libc.socket(addr.sa_family, libc.SOCK_STREAM, 0)
        if fd == -1:
            raise IOError("Cannot create socket")

        try:
            waiter = loop.create_future()
            handle = Handle(waiter.set_result, (None,), loop, None)
            connector.cb = &handle.cb

            if not ring_sq_submit_connect(
                    &loop.loop.ring.sq,
                    fd,
                    addr,
                    &connector.ring_cb,
            ):
                raise ValueError("Submission queue is full!")

            await waiter

            res = abs(connector.ring_cb.res)
            if res != 0:
                raise IOError(res, string.strerror(res))
            return fd

        except Exception as e:
            os.close(fd)
            exceptions.append(e)
    raise exceptions[0]


cdef int tcp_connect_cb(RingCallback* cb) nogil except 0:
    cdef TCPConnect* connector = <TCPConnect*>cb.data
    return queue_push(&connector.loop.ready, connector.cb)


cdef class TCPTransport:
    @staticmethod
    cdef TCPTransport new(int fd, object protocol, KLoopImpl loop):
        cdef TCPTransport rv = TCPTransport.__new__(TCPTransport)
        rv.fd = fd
        rv.protocol = protocol
        rv.loop = loop
        loop.call_soon(protocol.connection_made, rv)
        return rv

    def get_extra_info(self, x):
        return None
