# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef struct TCPConnect:
    int fd
    libc.sockaddr* addr
    RingCallback ring_cb
    Loop* loop
    Callback* cb


cdef class TCPTransport:
    cdef:
        KLoopImpl loop
        int fd
        TCPConnect connector
        object waiter
        object protocol_factory
        Handle handle

    @staticmethod
    cdef TCPTransport new(object protocol_factory, KLoopImpl loop)

    cdef connect(self, libc.sockaddr* addr)
    cdef connect_cb(self)
