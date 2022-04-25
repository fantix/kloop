# Copyright (c) 2022 Fantix King  http://fantix.pro
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
        &connector.addr,
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
        return rv

    cdef connect(self, host, port):
        cdef:
            int fd
            TCPConnect* c = &self.connector

        fd = libc.socket(socket.AF_INET, socket.SOCK_STREAM, 0)
        if fd == -1:
            PyErr_SetFromErrno(IOError)
            return
        self.host_bytes = host.encode()
        if not libc.inet_pton(
            socket.AF_INET, <char*>self.host_bytes, &c.addr.sin_addr
        ):
            PyErr_SetFromErrno(IOError)
            return
        c.addr.sin_family = socket.AF_INET
        c.addr.sin_port = libc.htons(port)
        c.fd = self.fd = fd
        c.loop = &self.loop.loop
        c.ring_cb.callback = tcp_connect_cb
        c.ring_cb.data = c
        self.handle = Handle(self.connect_cb, (self,), self.loop, None)
        c.cb = &self.handle.cb
        if not tcp_connect(c):
            raise ValueError("Submission queue is full!")
        self.waiter = self.loop.create_future()
        return self.waiter

    cdef connect_cb(self):
        if self.connector.ring_cb.res != 0:
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
