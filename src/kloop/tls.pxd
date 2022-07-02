# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


from cpython cimport PyObject
from .includes cimport libc
from .includes.openssl cimport bio
from .loop cimport KLoopImpl, Loop, RingCallback


cdef struct Proxy:
    PyObject* transport
    libc.iovec send_vec
    libc.msghdr send_msg
    RingCallback send_callback
    libc.iovec recv_vec
    libc.msghdr recv_msg
    RingCallback recv_callback
    unsigned char flags
    char* read_buffer
    void* cmsg

    Loop* loop
    int fd


cdef enum State:
    UNWRAPPED
    HANDSHAKING
    WRAPPED
    WRAPPED_KTLS


cdef class TLSTransport:
    cdef:
        KLoopImpl loop
        int fd
        bio.BIO* bio
        object protocol
        object sslctx
        object sslobj
        object waiter
        Proxy proxy
        State state
        object write_buffer
        bint sending


    cdef do_handshake(self)
    cdef do_read(self)
    cdef do_read_ktls(self)
    cdef write_cb(self, int res)
    cdef read_cb(self, int res)
