# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef int tcp_connect(TCPConnect* connector) nogil:
    return ring_sq_submit_connect(
        &connector.loop.ring.sq,
        connector.fd,
        connector.addr,
        &connector.ring_cb,
    )


cdef int tcp_connect_cb(RingCallback* cb) nogil except 0:
    cdef TCPConnect* connector = <TCPConnect*>cb.data
    return queue_push(&connector.loop.ready, connector.cb)


cdef class TCPTransport:
    @staticmethod
    cdef TCPTransport new(object protocol_factory, KLoopImpl loop):
        cdef TCPTransport rv = TCPTransport.__new__(TCPTransport)
        rv.protocol_factory = protocol_factory
        rv.loop = loop
        rv.connector.loop = &loop.loop
        rv.connector.ring_cb.callback = tcp_connect_cb
        rv.connector.ring_cb.data = &rv.connector
        return rv

    cdef connect(self, libc.sockaddr* addr):
        cdef:
            int fd
            TCPConnect* c = &self.connector

        fd = libc.socket(addr.sa_family, libc.SOCK_STREAM, 0)
        if fd == -1:
            PyErr_SetFromErrno(IOError)
            return
        c.addr = addr
        c.fd = self.fd = fd
        self.handle = Handle(self.connect_cb, (self,), self.loop, None)
        c.cb = &self.handle.cb
        if not tcp_connect(c):
            raise ValueError("Submission queue is full!")
        self.waiter = self.loop.create_future()
        return self.waiter

    cdef connect_cb(self):
        if self.connector.ring_cb.res != 0:
            if not ring_sq_submit_close(
                &self.loop.loop.ring.sq, self.fd, NULL
            ):
                # TODO: fd not closed?
                pass
            try:
                errno.errno = abs(self.connector.ring_cb.res)
                PyErr_SetFromErrno(IOError)
            except IOError as e:
                self.waiter.set_exception(e)
            return

        protocol = self.protocol_factory()
        self.waiter.set_result(protocol)
        self.loop.call_soon(protocol.connection_made, self)

    def get_extra_info(self, x):
        return None
