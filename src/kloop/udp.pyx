# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef extern int udp_bind(libc.sockaddr* addr, libc.socklen_t addrlen) nogil:
    cdef int fd = libc.socket(addr.sa_family, libc.SOCK_DGRAM, 0)
    if fd == -1:
        return -1
    if libc.bind(fd, addr, addrlen) == -1:
        # TODO: close fd
        return -1
    return fd


cdef extern unsigned long udp_send_init(int fd, CResolver* resolver) nogil:
    cdef UDPSend* rv
    rv = <UDPSend*>PyMem_RawMalloc(sizeof(UDPSend))
    if rv == NULL:
        return 0
    string.memset(rv, 0, sizeof(UDPSend))
    rv.fd = fd
    rv.resolver = resolver
    rv.msg.msg_iov = &rv.vec
    rv.msg.msg_iovlen = 1
    rv.callback.data = <void*>rv
    rv.callback.callback = udp_send_cb
    return <unsigned long>rv


cdef int udp_send_cb(RingCallback* cb) nogil except 0:
    cdef UDPSend* send = <UDPSend*>cb.data
    waker_wake(send.rust_waker)
    resolver_run_until_stalled(send.resolver.rust_resolver)
    return 1


cdef extern int udp_send_poll(
    unsigned long send_in,
    char* data,
    size_t datalen,
    libc.sockaddr* addr,
    libc.socklen_t addrlen,
    void* waker,
) nogil:
    cdef UDPSend* send = <UDPSend*>send_in
    if send.vec.iov_base == NULL:
        send.vec.iov_base = data
        send.vec.iov_len = datalen
        send.msg.msg_name = <void*>addr
        send.msg.msg_namelen = addrlen
        send.rust_waker = waker
        return ring_sq_submit_sendmsg(
            &send.resolver.loop.ring.sq,
            send.fd,
            &send.msg,
            &send.callback,
        ) - 1
    else:
        waker_forget(waker)
        if send.vec.iov_base != data or send.vec.iov_len != datalen:
            return -1
        return send.callback.res or -1
