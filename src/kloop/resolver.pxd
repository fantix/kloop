# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef extern from * nogil:
    int resolver_init(
        CResolver* resolver,
        char* resolv_conf_data,
        size_t resolv_conf_data_size,
        char* hosts_conf_data,
        size_t hosts_conf_data_size,
    )
    int resolver_lookup_ip(
        void* resolver,
        void* resolve,
        char* host,
        size_t length,
        libc.in_port_t port,
    )
    void resolver_run_until_stalled(void* rust_resolver)
    void waker_wake(void* waker)
    void waker_forget(void* waker)


cdef struct CResolver:
    Loop* loop
    Callback* cb
    FileReader resolv_conf
    FileReader hosts_conf
    int res
    void* rust_resolver


cdef class Resolver:
    cdef:
        CResolver resolver
        KLoopImpl loop
        Handle handle
        object waiter
        bint initialized

    @staticmethod
    cdef Resolver new(KLoopImpl loop)
    cdef init_cb(self)
    cdef err_cb(self, exc)


cdef struct CResolve:
    CResolver* resolver
    libc.sockaddr* result
    size_t result_len, result_size
    Callback* cb
    int res
    char* host
    size_t host_len
    libc.in_port_t port


cdef class Resolve:
    cdef:
        CResolve r
        Handle handle
        object waiter
        object host

    @staticmethod
    cdef new(Resolver resolver, host, port)
    cdef resolve_cb(self)
