/*
Copyright (c) 2022 Fantix King  http://fantix.pro
kLoop is licensed under Mulan PSL v2.
You can use this software according to the terms and conditions of the Mulan PSL v2.
You may obtain a copy of Mulan PSL v2 at:
         http://license.coscl.org.cn/MulanPSL2
THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
See the Mulan PSL v2 for more details.
*/


use core::marker;
use std::task::Waker;
use libc;

use crate::resolve::KLoopResolver;

#[repr(C)]
pub struct CResolver {
    _data: [u8; 0],
    _marker: marker::PhantomData<(*mut u8, marker::PhantomPinned)>,
}

#[repr(C)]
pub struct CResolve {
    _data: [u8; 0],
    _marker: marker::PhantomData<(*mut u8, marker::PhantomPinned)>,
}

#[repr(C)]
pub struct UDPSend {
    _data: [u8; 0],
    _marker: marker::PhantomData<(*mut u8, marker::PhantomPinned)>,
}

extern "C" {
    pub fn resolver_set(c_resolver: *const CResolver, resolver: *mut KLoopResolver);
    // pub fn resolve_set_poller(resolve: *const CResolve, poller: *mut Poller);
    pub fn resolve_prep_addr(resolve: *const CResolve) -> *mut libc::sockaddr;
    pub fn resolve_done_cb(resolve: *const CResolve) -> libc::c_int;
    pub fn udp_bind(addr: *const libc::sockaddr, addrlen: libc::socklen_t) -> libc::c_int;
    pub fn udp_send_init(fd: libc::c_int, resolver: *const CResolver) -> libc::c_ulong;
    pub fn udp_send_poll(
        send: libc::c_ulong,
        data: *const u8,
        datalen: libc::size_t,
        addr: *const libc::sockaddr,
        addrlen: libc::socklen_t,
        waker: *mut Waker,
    ) -> libc::c_int;
}
