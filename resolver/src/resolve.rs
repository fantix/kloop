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


use futures_executor::{LocalPool, LocalSpawner};
use futures_util::task::{LocalSpawnExt, SpawnError};
use std;
use std::cell::RefCell;
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::rc::Rc;

use libc;
use trust_dns_resolver::name_server::{GenericConnection, GenericConnectionProvider};
use trust_dns_resolver::system_conf::parse_resolv_conf;
use trust_dns_resolver::{AsyncResolver, Hosts};

use crate::kloop::{resolve_done_cb, resolve_prep_addr, resolver_set, CResolve, CResolver};
use crate::runtime::{KLoopHandle, KLoopRuntime};

const DEFAULT_PORT: u16 = 53;

type KLoopConnection = GenericConnection;

type KLoopConnectionProvider = GenericConnectionProvider<KLoopRuntime>;

thread_local! {
    pub static CURRENT_RESOLVER: Rc<RefCell<Option<*mut KLoopResolver>>> = Rc::new(RefCell::new(None));
}

#[derive(Debug)]
pub struct KLoopResolver {
    resolver: AsyncResolver<KLoopConnection, KLoopConnectionProvider>,
    pub c_resolver: *const CResolver,
    pool: LocalPool,
    spawner: LocalSpawner,
}

impl KLoopResolver {
    fn new(
        resolv_conf: &[u8],
        hosts_conf: &[u8],
        c_resolver: *const CResolver,
    ) -> io::Result<Self> {
        let (config, mut options) = parse_resolv_conf(resolv_conf)?;
        options.use_hosts_file = false;
        let conn_provider = GenericConnectionProvider::new(KLoopHandle);
        let mut resolver = AsyncResolver::new_with_conn(config, options, conn_provider).unwrap();
        resolver.set_hosts(Some(Hosts::default().read_hosts_conf(hosts_conf)));
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        Ok(Self {
            resolver,
            c_resolver,
            pool,
            spawner,
        })
    }

    pub fn spawn<Fut>(&self, future: Fut) -> Result<(), SpawnError>
    where
        Fut: Future<Output = ()> + 'static,
    {
        self.spawner.spawn_local(future)
    }

    fn run_until_stalled(&mut self) {
        loop {
            self.pool.run_until_stalled();
            if !self.pool.try_run_one() {
                break;
            }
        }
    }

    async fn lookup_ip(&self, resolve: *mut CResolve, host: &str, port: libc::in_port_t) -> () {
        match self.resolver.lookup_ip(host).await {
            Ok(result) => {
                for ip in result.into_iter() {
                    match ip {
                        IpAddr::V4(ip) => unsafe {
                            let out = resolve_prep_addr(resolve) as *mut libc::sockaddr_in;
                            if out.is_null() {
                                println!("resolve_prep_addr returned NULL");
                                break;
                            }
                            (*out).sin_family = libc::AF_INET as libc::sa_family_t;
                            (*out).sin_addr = libc::in_addr {
                                s_addr: u32::from_ne_bytes(ip.octets()),
                            };
                            (*out).sin_port = port.to_be();
                            (*out).sin_zero = [0; 8];
                        },
                        IpAddr::V6(ip) => unsafe {
                            let out = resolve_prep_addr(resolve) as *mut libc::sockaddr_in6;
                            if out.is_null() {
                                println!("resolve_prep_addr returned NULL");
                                break;
                            }
                            (*out).sin6_family = libc::AF_INET6 as libc::sa_family_t;
                            (*out).sin6_addr = libc::in6_addr {
                                s6_addr: ip.octets(),
                            };
                            (*out).sin6_port = port.to_be();
                        },
                    }
                }
            }
            Err(e) => {
                println!("lookup_ip error: {:?}", e);
            }
        }
        unsafe {
            resolve_done_cb(resolve);
        }
    }
}

#[no_mangle]
pub extern "C" fn resolver_init(
    c_resolver: *const CResolver,
    resolv_conf_data: *const u8,
    resolv_conf_data_size: libc::size_t,
    hosts_conf_data: *const u8,
    hosts_conf_data_size: libc::size_t,
) -> libc::c_int {
    let resolv_conf =
        unsafe { std::slice::from_raw_parts(resolv_conf_data, resolv_conf_data_size) };
    let hosts_conf = unsafe { std::slice::from_raw_parts(hosts_conf_data, hosts_conf_data_size) };
    let mut resolver = match KLoopResolver::new(resolv_conf, hosts_conf, c_resolver) {
        Ok(resolver) => resolver,
        Err(e) => return 0,
    };
    let rv = Box::into_raw(Box::new(resolver));
    unsafe {
        resolver_set(c_resolver, rv);
    }
    1
}

#[no_mangle]
pub extern "C" fn resolver_lookup_ip(
    resolver: *mut KLoopResolver,
    resolve: *mut CResolve,
    host_raw: *const u8,
    length: libc::size_t,
    port: libc::in_port_t,
) -> libc::c_int {
    let host = match std::str::from_utf8(unsafe { std::slice::from_raw_parts(host_raw, length) }) {
        Ok(host) => host,
        _ => return 0,
    };
    let r = || unsafe { resolver.as_mut() }.unwrap();

    let fut = r().lookup_ip(resolve, host, port);
    if let Err(e) = r().spawn(fut) {
        println!("spawn error: {:?}", e);
        return 0;
    }
    CURRENT_RESOLVER.with(|current| {
        *current.borrow_mut() = Some(resolver);
        r().run_until_stalled();
        *current.borrow_mut() = None;
    });
    1
}

#[no_mangle]
pub extern "C" fn resolver_run_until_stalled(resolver: *mut KLoopResolver) {
    CURRENT_RESOLVER.with(|current| {
        *current.borrow_mut() = Some(resolver);
        unsafe { resolver.as_mut() }.unwrap().run_until_stalled();
        *current.borrow_mut() = None;
    });
}
