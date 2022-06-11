/*
Copyright (c) 2022 Fantix King  https://fantix.pro
kLoop is licensed under Mulan PSL v2.
You can use this software according to the terms and conditions of the Mulan PSL v2.
You may obtain a copy of Mulan PSL v2 at:
         http://license.coscl.org.cn/MulanPSL2
THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
See the Mulan PSL v2 for more details.
*/


use crate::kloop;
use crate::resolve::CURRENT_RESOLVER;
use async_trait::async_trait;
use futures_io::{AsyncRead, AsyncWrite};
use libc::{sockaddr, socklen_t};
use std::fmt::Debug;
use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use std::{io, mem};
use trust_dns_proto::error::ProtoError;
use trust_dns_proto::tcp::{Connect, DnsTcpStream};
use trust_dns_proto::udp::UdpSocket;
use trust_dns_proto::Time;
use trust_dns_resolver::name_server::{RuntimeProvider, Spawn};

#[derive(Clone, Copy, Debug)]
pub struct KLoopTimer {}

#[async_trait]
impl Time for KLoopTimer {
    async fn delay_for(duration: Duration) {
        println!("TODO: delay_for: {:?}", duration);
    }

    async fn timeout<F: 'static + Future + Send>(
        duration: Duration,
        future: F,
    ) -> io::Result<F::Output> {
        println!("TODO: timeout: {:?}", duration);
        Ok(future.await)
    }
}

pub struct KLoopTcp {}

#[async_trait]
impl Connect for KLoopTcp {
    async fn connect_with_bind(
        addr: SocketAddr,
        bind_addr: Option<SocketAddr>,
    ) -> io::Result<Self> {
        println!("TODO: connect_with_bind: {:?} {:?}", addr, bind_addr);
        todo!()
    }
}

impl AsyncRead for KLoopTcp {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        println!("TODO: poll_read");
        todo!()
    }
}

impl AsyncWrite for KLoopTcp {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        println!("TODO: poll_write");
        todo!()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        println!("TODO: poll_flush");
        todo!()
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        println!("TODO: poll_close");
        todo!()
    }
}

impl DnsTcpStream for KLoopTcp {
    type Time = KLoopTimer;
}

pub struct KLoopUdp {
    fd: libc::c_int,
    send: libc::c_ulong,
    recv: libc::c_ulong,
}

#[async_trait]
impl UdpSocket for KLoopUdp {
    type Time = KLoopTimer;

    async fn connect(addr: SocketAddr) -> io::Result<Self> {
        println!("TODO: KLoopUdp: connect({})", addr);
        todo!()
    }

    async fn connect_with_bind(addr: SocketAddr, bind_addr: SocketAddr) -> io::Result<Self> {
        println!("TODO: KLoopUdp: connect_with_bind({}, {})", addr, bind_addr);
        todo!()
    }

    async fn bind(addr: SocketAddr) -> io::Result<Self> {
        let (addr_ptr, addr_len) = socket_addr_as_ptr(addr);
        let fd = unsafe { kloop::udp_bind(addr_ptr, addr_len) };
        CURRENT_RESOLVER.with(|resolver| {
            let resolver = resolver.borrow().unwrap();
            let resolver = unsafe { resolver.as_ref() }.unwrap();
            let resolver = resolver.c_resolver;
            let send = unsafe { kloop::udp_action_init(fd, resolver) };
            let recv = unsafe { kloop::udp_action_init(fd, resolver) };
            Ok(KLoopUdp { fd, send, recv })
        })
    }

    fn poll_recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<(usize, SocketAddr)>> {
        let waker = Box::new(cx.waker().clone());
        match unsafe {
            kloop::udp_recv_poll(
                self.recv,
                buf.as_mut_ptr(),
                buf.len(),
                Box::into_raw(waker),
            )
        } {
            res if res > 0 => {
                Poll::Ready(unsafe {
                    ptr_to_socket_addr(kloop::udp_get_addr(self.recv))
                }.map(|addr| (res as usize, addr)))
            }
            _ => Poll::Pending,
        }
    }

    fn poll_send_to(
        &self,
        cx: &mut Context<'_>,
        buf: &[u8],
        target: SocketAddr,
    ) -> Poll<io::Result<usize>> {
        let waker = Box::new(cx.waker().clone());
        let (addr, addrlen) = socket_addr_as_ptr(target);
        match unsafe {
            kloop::udp_send_poll(
                self.send,
                buf.as_ptr(),
                buf.len(),
                addr,
                addrlen,
                Box::into_raw(waker),
            )
        } {
            res if res > 0 => {
                Poll::Ready(Ok(res as usize))
            }
            res => {
                Poll::Pending
            }
        }
    }
}

impl Drop for KLoopUdp {
    fn drop(&mut self) {
        unsafe {
            kloop::udp_action_free(self.send);
            kloop::udp_action_free(self.recv);
        }
    }
}

fn socket_addr_as_ptr(addr: SocketAddr) -> (*const sockaddr, socklen_t) {
    match addr {
        SocketAddr::V4(ref a) => (
            a as *const _ as *const _,
            mem::size_of_val(a) as libc::socklen_t,
        ),
        SocketAddr::V6(ref a) => (
            a as *const _ as *const _,
            mem::size_of_val(a) as libc::socklen_t,
        ),
    }
}


unsafe fn ptr_to_socket_addr(
    addr: *const libc::sockaddr,
) -> io::Result<SocketAddr> {
    match (*addr).sa_family as libc::c_int {
        libc::AF_INET => {
            let addr: &libc::sockaddr_in = &*(addr as *const libc::sockaddr_in);
            let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
            let port = u16::from_be(addr.sin_port);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        libc::AF_INET6 => {
            let addr: &libc::sockaddr_in6 = &*(addr as *const libc::sockaddr_in6);
            let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
            let port = u16::from_be(addr.sin6_port);
            Ok(SocketAddr::V6(SocketAddrV6::new(
                ip,
                port,
                addr.sin6_flowinfo,
                addr.sin6_scope_id,
            )))
        }
        _ => Err(io::ErrorKind::InvalidInput.into()),
    }
}

#[derive(Clone, Copy)]
pub struct KLoopHandle;

impl Spawn for KLoopHandle {
    fn spawn_bg<F>(&mut self, future: F)
    where
        F: Future<Output = Result<(), ProtoError>> + Send + 'static,
    {
        CURRENT_RESOLVER.with(|resolver| {
            let r = resolver.borrow().unwrap();
            let r = unsafe { r.as_mut() }.unwrap();
            r.spawn(async {
                future.await.unwrap_or_else(|e| {
                    println!("spawn_bg error: {:?}", e);
                });
            });
        });
    }
}

#[derive(Clone, Copy)]
pub struct KLoopRuntime;

impl RuntimeProvider for KLoopRuntime {
    type Handle = KLoopHandle;
    type Timer = KLoopTimer;
    type Udp = KLoopUdp;
    type Tcp = KLoopTcp;
}

#[no_mangle]
pub extern "C" fn waker_wake(waker: *mut Waker) {
    let waker = unsafe { Box::from_raw(waker) };
    waker.wake();
}

#[no_mangle]
pub extern "C" fn waker_forget(waker: *mut Waker) {
    unsafe { Box::from_raw(waker) };
}
