# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


import collections
import socket
import ssl
from cpython cimport PyMem_RawMalloc, PyMem_RawFree
from libc cimport errno, string

from .includes.openssl cimport err, ssl as ssl_h
from .includes cimport pyssl, linux
from .loop cimport ring_sq_submit_sendmsg, ring_sq_submit_recvmsg


cdef int BIO_CTRL_SET_KTLS = 72
cdef int BIO_CTRL_GET_KTLS_SEND = 73
cdef int BIO_CTRL_GET_KTLS_RECV = 76

cdef int FLAGS_KTLS_TX_CTRL_MSG = 0x1000
cdef int FLAGS_KTLS_RX = 0x2000
cdef int FLAGS_KTLS_TX = 0x4000

cdef unsigned char FLAGS_PROXY_SEND_SUBMITTED = 1 << 0
cdef unsigned char FLAGS_PROXY_SEND_COMPLETED = 1 << 1
cdef unsigned char FLAGS_PROXY_SEND_IN_PROXY = 1 << 2
cdef unsigned char FLAGS_PROXY_SEND_ALL = (
    FLAGS_PROXY_SEND_SUBMITTED |
    FLAGS_PROXY_SEND_COMPLETED |
    FLAGS_PROXY_SEND_IN_PROXY
)
cdef unsigned char FLAGS_PROXY_RECV_SUBMITTED = 1 << 4
cdef unsigned char FLAGS_PROXY_RECV_COMPLETED = 1 << 5
cdef unsigned char FLAGS_PROXY_RECV_KTLS = 1 << 6
cdef unsigned char FLAGS_PROXY_RECV_ALL = (
    FLAGS_PROXY_RECV_SUBMITTED |
    FLAGS_PROXY_RECV_COMPLETED
)

cdef size_t CMSG_SIZE = libc.CMSG_SPACE(sizeof(unsigned char))
DEF DEBUG = 0


cdef inline void reset_msg(libc.msghdr* msg, void* cmsg) nogil:
    msg.msg_name = NULL
    msg.msg_namelen = 0
    msg.msg_flags = 0
    if cmsg == NULL:
        msg.msg_control = NULL
        msg.msg_controllen = 0
    else:
        msg.msg_control = cmsg
        msg.msg_controllen = CMSG_SIZE


cdef object fromOpenSSLError(object err_type):
    cdef:
        unsigned long e = err.get_error()
        const char* msg = err.reason_error_string(e)
    if msg == NULL:
        return err_type()
    else:
        return err_type(msg.decode("ISO-8859-1"))


cdef int bio_write_ex(
    bio.BIO* b, const char* data, size_t datal, size_t* written
) nogil:
    cdef:
        Proxy* proxy = <Proxy*>bio.get_data(b)
        int res

    IF DEBUG:
        with gil:
            print("bio_write_ex(data=%x, datal=%d)" % (<long long>data, datal))

    if proxy.flags & FLAGS_PROXY_SEND_SUBMITTED:
        if proxy.send_vec.iov_base != data:
            IF DEBUG:
                with gil:
                    print("bio_write_ex() error: concurrent call")
            return 0
        if proxy.send_vec.iov_len > datal:
            IF DEBUG:
                with gil:
                    print("bio_write_ex() error: short rewrite")
            return 0

    bio.clear_retry_flags(b)
    if proxy.flags & FLAGS_PROXY_SEND_COMPLETED:
        proxy.flags &= ~FLAGS_PROXY_SEND_ALL
        res = proxy.send_callback.res
        if res < 0:
            IF DEBUG:
                with gil:
                    print("bio_write_ex() error:", -res)
            errno.errno = -res
            return 0

        written[0] = res
        IF DEBUG:
            with gil:
                print("bio_write_ex() written:", res)
                print(">>> ", end="")
                for i in range(res):
                    print(
                         "%02x " % <unsigned char>data[i],
                         end="" if (i + 1) % 16 or i == res - 1 else "\n>>> ",
                    )
                print()

    else:
        written[0] = 0
        bio.set_retry_write(b)

        if not proxy.flags & FLAGS_PROXY_SEND_SUBMITTED:
            IF DEBUG:
                with gil:
                    print("bio_write_ex() submit")
            proxy.send_vec.iov_base = data
            proxy.send_vec.iov_len = datal
            reset_msg(&proxy.send_msg, NULL)
            if not ring_sq_submit_sendmsg(
                &proxy.loop.ring.sq,
                proxy.fd,
                &proxy.send_msg,
                &proxy.send_callback,
            ):
                IF DEBUG:
                    with gil:
                        print("bio_write_ex() error: SQ full")
                return 0
            proxy.flags |= FLAGS_PROXY_SEND_SUBMITTED

    return 1


cdef int bio_read_ex(
    bio.BIO* b, char* data, size_t datal, size_t* readbytes
) nogil:
    cdef:
        Proxy* proxy = <Proxy*>bio.get_data(b)
        libc.cmsghdr* cmsg = NULL
        int res
        int is_ktls = bio.test_flags(b, FLAGS_KTLS_RX)

    IF DEBUG:
        with gil:
            print('bio_read_ex(data=%x, datal=%d)' % (<long long>data, datal))

    if proxy.flags & FLAGS_PROXY_RECV_SUBMITTED:
        if proxy.recv_vec.iov_base != (data + 5 if is_ktls else data):
            IF DEBUG:
                with gil:
                    print("bio_read_ex() error: concurrent call")
            return 0
        if proxy.recv_vec.iov_len > (datal - 21 if is_ktls else datal):
            IF DEBUG:
                with gil:
                    print("bio_read_ex() error: short reread")
            return 0

    bio.clear_retry_flags(b)
    if (
        proxy.flags & FLAGS_PROXY_RECV_KTLS and
        proxy.flags & FLAGS_PROXY_RECV_COMPLETED
    ):
        proxy.flags &= ~FLAGS_PROXY_RECV_ALL
        res = proxy.recv_callback.res
        if datal < res + 5:
            IF DEBUG:
                with gil:
                    print("bio_read_ex() error: datal too short")
            errno.errno = errno.EINVAL
            return 0

        cmsg = libc.CMSG_FIRSTHDR(&proxy.recv_msg)
        if cmsg.cmsg_type == linux.TLS_GET_RECORD_TYPE:
            data[0] = (<unsigned char *> libc.CMSG_DATA(cmsg))[0]
            data[1] = 0x03  # TLS1_2_VERSION_MAJOR
            data[2] = 0x03  # TLS1_2_VERSION_MINOR
            # returned length is limited to msg_iov.iov_len above
            data[3] = (res >> 8) & 0xff
            data[4] = res & 0xff
            string.memcpy(data + 5, proxy.read_buffer, res)
            res += 5
        else:
            string.memcpy(data, proxy.read_buffer, res)
        readbytes[0] = res

        IF DEBUG:
            with gil:
                print("bio_read_ex() read:", res, "(forwarded TLS record)")
                print("<<< ", end="")
                for i in range(res):
                    print(
                        "%02x " % <unsigned char>data[i],
                        end="" if (i + 1) % 16 or i == res - 1 else "\n<<< ",
                    )
                print()

    elif proxy.flags & FLAGS_PROXY_RECV_COMPLETED:
        proxy.flags &= ~FLAGS_PROXY_RECV_ALL
        res = proxy.recv_callback.res
        if res < 0:
            IF DEBUG:
                with gil:
                    print("bio_read_ex() error:", -res)
            errno.errno = -res
            return 0

        if is_ktls:
            if proxy.recv_msg.msg_controllen:
                cmsg = libc.CMSG_FIRSTHDR(&proxy.recv_msg)
                if cmsg.cmsg_type == linux.TLS_GET_RECORD_TYPE:
                    data[0] = (<unsigned char*>libc.CMSG_DATA(cmsg))[0]
                    data[1] = 0x03  # TLS1_2_VERSION_MAJOR
                    data[2] = 0x03  # TLS1_2_VERSION_MINOR
                    # returned length is limited to msg_iov.iov_len above
                    data[3] = (res >> 8) & 0xff
                    data[4] = res & 0xff
                    res += 5

        if res == 0:
            bio.set_flags(b, bio.FLAGS_IN_EOF)
        readbytes[0] = res

        IF DEBUG:
            with gil:
                print(
                    "bio_read_ex() read:", res, "(TLS record)" if cmsg else ""
                )
                print("<<< ", end="")
                for i in range(res):
                    print(
                        "%02x " % <unsigned char>data[i],
                        end="" if (i + 1) % 16 or i == res - 1 else "\n<<< ",
                    )
                print()
    else:
        bio.set_retry_read(b)
        readbytes[0] = 0
        if not proxy.flags & FLAGS_PROXY_RECV_SUBMITTED:
            if proxy.flags & FLAGS_PROXY_RECV_KTLS:
                reset_msg(&proxy.recv_msg, proxy.cmsg)
                IF DEBUG:
                    with gil:
                        print("bio_read_ex() submit(%x, %d)" % (
                            <long long>proxy.recv_vec.iov_base,
                            proxy.recv_vec.iov_len,
                        ))
            elif is_ktls:
                if datal < 21:
                    IF DEBUG:
                        with gil:
                            print("bio_read_ex() error: datal too short")
                    errno.errno = errno.EINVAL
                    return 0

                proxy.recv_vec.iov_base = data + 5
                proxy.recv_vec.iov_len = datal - 21
                reset_msg(&proxy.recv_msg, proxy.cmsg)
                IF DEBUG:
                    with gil:
                        print("bio_read_ex() submit(%x, %d)" % (
                            <long long>proxy.recv_vec.iov_base,
                            proxy.recv_vec.iov_len,
                        ))
            else:
                proxy.recv_vec.iov_base = data
                proxy.recv_vec.iov_len = datal
                reset_msg(&proxy.recv_msg, NULL)
                IF DEBUG:
                    with gil:
                        print("bio_read_ex() submit")

            if not ring_sq_submit_recvmsg(
                &proxy.loop.ring.sq,
                proxy.fd,
                &proxy.recv_msg,
                &proxy.recv_callback,
            ):
                IF DEBUG:
                    with gil:
                        print("bio_read_ex() error: SQ full")
                return 0
            proxy.flags |= FLAGS_PROXY_RECV_SUBMITTED

    return 1


cdef long bio_ctrl(bio.BIO* b, int cmd, long num, void* ptr) nogil:
    cdef:
        ssl_h.ktls_crypto_info_t* crypto_info
        long ret = 0
    if cmd == bio.BIO_CTRL_EOF:
        IF DEBUG:
            with gil:
                print("BIO_CTRL_EOF", ret)
    elif cmd == bio.BIO_CTRL_PUSH:
        IF DEBUG:
            with gil:
                print("BIO_CTRL_PUSH", ret)
    elif cmd == bio.BIO_CTRL_POP:
        IF DEBUG:
            with gil:
                print("BIO_CTRL_POP", ret)
    elif cmd == bio.BIO_CTRL_FLUSH:
        ret = 1
        IF DEBUG:
            with gil:
                print('BIO_CTRL_FLUSH', ret)
    elif cmd == BIO_CTRL_SET_KTLS:
        IF DEBUG:
            with gil:
                print("BIO_CTRL_SET_KTLS", "TX end" if num else "RX end")
        crypto_info = <ssl_h.ktls_crypto_info_t*>ptr
        if libc.setsockopt(
            (<Proxy*>bio.get_data(b)).fd,
            libc.SOL_TLS,
            linux.TLS_TX if num else linux.TLS_RX,
            crypto_info,
            crypto_info.tls_crypto_info_len,
        ) == 0:
            bio.set_flags(b, FLAGS_KTLS_TX if num else FLAGS_KTLS_RX)
        else:
            IF DEBUG:
                with gil:
                    print(
                        "BIO_CTRL_SET_KTLS",
                        "TX end" if num else "RX end",
                        "failed",
                    )
    elif cmd == BIO_CTRL_GET_KTLS_SEND:
        return bio.test_flags(b, FLAGS_KTLS_TX) != 0
    elif cmd == BIO_CTRL_GET_KTLS_RECV:
        return bio.test_flags(b, FLAGS_KTLS_RX) != 0
    else:
        IF DEBUG:
            with gil:
                print('bio_ctrl', cmd, num)
    return ret


cdef int bio_create(bio.BIO* b) nogil:
    bio.set_init(b, 1)
    return 1


cdef int bio_destroy(bio.BIO* b) nogil:
    bio.set_shutdown(b, 1)
    return 1


cdef int tls_send_cb(RingCallback* cb) nogil except 0:
    cdef Proxy* proxy = <Proxy*>cb.data
    proxy.flags |= FLAGS_PROXY_SEND_COMPLETED
    with gil:
        (<TLSTransport>proxy.transport).write_cb(cb.res)
    return 1


cdef int tls_recv_cb(RingCallback* cb) nogil except 0:
    cdef Proxy* proxy = <Proxy*>cb.data
    proxy.flags |= FLAGS_PROXY_RECV_COMPLETED
    with gil:
        (<TLSTransport>proxy.transport).read_cb(cb.res)
    return 1


cdef class TLSTransport:
    @staticmethod
    def new(
        int fd,
        protocol,
        KLoopImpl loop,
        sslctx,
        server_side=False,
        server_hostname=None,
        session=None,
        waiter=None,
    ):
        cdef:
            TLSTransport rv = TLSTransport.__new__(TLSTransport)
            pyssl.PySSLMemoryBIO* c_bio
            pyssl.SSL* s

        libc.setsockopt(fd, socket.SOL_TCP, linux.TCP_ULP, b"tls", 3)

        py_bio = ssl.MemoryBIO()
        c_bio = <pyssl.PySSLMemoryBIO*>py_bio
        c_bio.bio, rv.bio = rv.bio, c_bio.bio
        try:
            rv.sslobj = sslctx.wrap_bio(
                py_bio, py_bio, server_side, server_hostname, session
            )
        finally:
            c_bio.bio, rv.bio = rv.bio, c_bio.bio
            del py_bio

        s = (<pyssl.PySSLSocket*>rv.sslobj._sslobj).ssl
        ssl_h.set_options(s, ssl_h.OP_ENABLE_KTLS)
        ssl_h.clear_mode(
            s,
            ssl_h.SSL_MODE_RELEASE_BUFFERS |
            ssl_h.SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER
        )
        rv.fd = fd
        rv.protocol = protocol
        rv.loop = loop
        rv.sslctx = sslctx
        rv.proxy.loop = &loop.loop
        rv.proxy.fd = fd
        rv.waiter = waiter
        rv.write_buffer = collections.deque()

        rv.do_handshake()
        return rv

    def __cinit__(self):
        self.state = UNWRAPPED
        self.bio = bio.new(KTLS_BIO_METHOD)
        self.proxy.transport = <PyObject*>self
        self.proxy.send_msg.msg_iov = &self.proxy.send_vec
        self.proxy.send_msg.msg_iovlen = 1
        self.proxy.send_callback.data = <void*>&self.proxy
        self.proxy.send_callback.callback = tls_send_cb
        self.proxy.cmsg = PyMem_RawMalloc(CMSG_SIZE)
        if self.proxy.cmsg == NULL:
            raise MemoryError
        self.proxy.recv_msg.msg_iov = &self.proxy.recv_vec
        self.proxy.recv_msg.msg_iovlen = 1
        self.proxy.recv_callback.data = <void*>&self.proxy
        self.proxy.recv_callback.callback = tls_recv_cb
        bio.set_data(self.bio, <void*>&self.proxy)

    def __dealloc__(self):
        self.sslobj = None
        bio.free(self.bio)
        PyMem_RawFree(self.proxy.read_buffer)
        PyMem_RawFree(self.proxy.cmsg)

    cdef do_handshake(self):
        if self.state == UNWRAPPED:
            self.state = HANDSHAKING
        elif self.state != HANDSHAKING:
            raise RuntimeError("Cannot do handshake now")


        try:
            IF DEBUG:
                print("do_handshake()")
            self.sslobj.do_handshake()
        except ssl.SSLWantReadError:
            IF DEBUG:
                print("do_handshake() SSLWantReadError")
        except ssl.SSLWantWriteError:
            IF DEBUG:
                print("do_handshake() SSLWantWriteError")
        except Exception as ex:
            IF DEBUG:
                print('do_handshake() error:', ex)
            raise
        else:
            IF DEBUG:
                print('do_handshake() done')

            self.state = WRAPPED
            if self.waiter:
                self.waiter.set_result(self)
                self.waiter = None

            if bio.test_flags(self.bio, FLAGS_KTLS_RX):
                self.proxy.read_buffer = <char*>PyMem_RawMalloc(65536)
                if self.proxy.read_buffer == NULL:
                    raise MemoryError
                self.proxy.flags |= FLAGS_PROXY_RECV_KTLS
                self.proxy.recv_vec.iov_base = self.proxy.read_buffer
                self.proxy.recv_vec.iov_len = 65536
                self.do_read_ktls()
            else:
                self.do_read()

    cdef do_read_ktls(self):
        cdef:
            int res
            libc.cmsghdr* cmsg
            unsigned char record_type

        if self.proxy.flags & FLAGS_PROXY_RECV_COMPLETED:
            self.proxy.flags &= ~FLAGS_PROXY_RECV_ALL
            res = self.proxy.recv_callback.res
            if res < 0:
                IF DEBUG:
                    print("do_read_ktls() error:", -res)
                self.loop.call_soon(
                    self.protocol.connection_lost,
                    IOError(-res, string.strerror(-res))
                )
            elif res == 0:
                IF DEBUG:
                    print("do_read_ktls() EOF")
                self.loop.call_soon(self.protocol.eof_received)
                self.loop.call_soon(self.protocol.connection_lost, None)
            else:
                if self.proxy.recv_msg.msg_controllen:
                    cmsg = libc.CMSG_FIRSTHDR(&self.proxy.recv_msg)
                    if cmsg.cmsg_type == linux.TLS_GET_RECORD_TYPE:
                        record_type = (<unsigned char*>libc.CMSG_DATA(cmsg))[0]
                        if record_type != ssl_h.SSL3_RT_APPLICATION_DATA:
                            IF DEBUG:
                                print("do_read_ktls() forward CMSG")
                            self.proxy.flags |= FLAGS_PROXY_RECV_COMPLETED
                            return self.do_read()
                IF DEBUG:
                    print("do_read_ktls() received", res, "bytes")
                self.loop.call_soon(
                    self.protocol.data_received,
                    bytes(self.proxy.read_buffer[:res]),
                )
                self.loop.call_soon(self.do_read_ktls, self)

        elif not self.proxy.flags & FLAGS_PROXY_RECV_SUBMITTED:
            IF DEBUG:
                print("do_read_ktls() submit")
            reset_msg(&self.proxy.recv_msg, self.proxy.cmsg)
            if not ring_sq_submit_recvmsg(
                    &self.proxy.loop.ring.sq,
                    self.fd,
                    &self.proxy.recv_msg,
                    &self.proxy.recv_callback,
            ):
                raise RuntimeError("SQ full")
            self.proxy.flags |= FLAGS_PROXY_RECV_SUBMITTED

    cdef do_read(self):
        try:
            data = self.sslobj.read(65536)
        except ssl.SSLWantReadError:
            IF DEBUG:
                print("do_read() SSLWantReadError")
        except ssl.SSLWantWriteError:
            IF DEBUG:
                print("do_read() SSLWantWriteError")
        except Exception as ex:
            IF DEBUG:
                print("do_read() error:", ex)
            self.loop.call_soon(self.protocol.connection_lost, ex)
        else:
            if data:
                IF DEBUG:
                    print("do_read() received", len(data), bytes)
                    print(data)
                self.loop.call_soon(self.protocol.data_received, data)
                if self.proxy.flags & FLAGS_PROXY_RECV_KTLS:
                    self.loop.call_soon(self.do_read_ktls, self)
                else:
                    self.loop.call_soon(self.do_read, self)
            else:
                IF DEBUG:
                    print("do_read() EOF")
                self.loop.call_soon(self.protocol.eof_received)
                self.loop.call_soon(self.protocol.connection_lost, None)

    cdef write_cb(self, int res):
        IF DEBUG:
            print("write_cb", res, "state:", self.state)
        if self.state == HANDSHAKING:
            self.do_handshake()

    cdef read_cb(self, int res):
        IF DEBUG:
            print("read_cb", res, "state:", self.state)
        if self.state == HANDSHAKING:
            self.do_handshake()
        elif self.state == WRAPPED:
            if self.proxy.flags & FLAGS_PROXY_RECV_KTLS:
                self.do_read_ktls()
            else:
                self.do_read()

    def write(self, data):
        if self.sending:
            self.write_buffer.append(data)
        else:
            try:
                self.sslobj.write(data)
            except ssl.SSLWantWriteError:
                IF DEBUG:
                    print("write() SSLWantWriteError")


cdef bio.Method* KTLS_BIO_METHOD = bio.meth_new(
    bio.get_new_index(), "kTLS BIO"
)
if not bio.meth_set_write_ex(KTLS_BIO_METHOD, bio_write_ex):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_read_ex(KTLS_BIO_METHOD, bio_read_ex):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_ctrl(KTLS_BIO_METHOD, bio_ctrl):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_create(KTLS_BIO_METHOD, bio_create):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_destroy(KTLS_BIO_METHOD, bio_destroy):
    raise fromOpenSSLError(ImportError)
