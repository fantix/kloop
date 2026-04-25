// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2026 Fantix King

use std::{
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4},
    sync::Arc,
};

use compio::{
    buf::buf_try,
    driver::{AsRawFd, op},
    tls::{
        TlsAcceptor, TlsConnector,
        py_dynamic_openssl::{self, SSLContext},
        rustls::{self, pki_types::CertificateDer},
    },
};
use pyo3::{
    IntoPyObjectExt,
    exceptions::{PyOSError, PyTypeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyList},
};
use socket2::{Domain, Protocol, Type};

use super::{
    Socket, SocketStream, idna_converter, name_to_ip,
    ssl::{RustlsContext, SSLImpl, SSLSocket, SSLSocketMetadata},
};
use crate::{Either, event_loop::CompioLoop, extract_py_err, import, py_any_to_buffer};

#[pyclass(unsendable, name = "Socket")]
pub struct PySocket {
    pyloop: Py<CompioLoop>,
    domain: Domain,
    ty: Type,
    protocol: Option<Protocol>,
    inner: Option<Socket>,
    bound: bool,
}

impl PySocket {
    pub async fn new(
        pyloop: Py<CompioLoop>,
        domain: Domain,
        ty: Type,
        protocol: Option<Protocol>,
    ) -> PyResult<Py<PyAny>> {
        let inner = Some(Socket::new(domain, ty, protocol).await?);
        Python::attach(|py| {
            Bound::new(
                py,
                Self {
                    pyloop,
                    domain,
                    ty,
                    protocol,
                    inner,
                    bound: false,
                },
            )?
            .into_py_any(py)
        })
    }

    #[inline]
    fn inner(&self) -> PyResult<&Socket> {
        self.inner
            .as_ref()
            .ok_or_else(|| PyOSError::new_err("socket is closed"))
    }
}

#[pymethods]
impl PySocket {
    fn __repr__(&self) -> PyResult<String> {
        if let Some(inner) = &self.inner {
            let fd = inner.as_raw_fd();
            let family = i32::from(self.domain);
            let ty = i32::from(self.ty);
            let proto = self.protocol.map(i32::from).unwrap_or_default();
            let laddr = match inner.socket.local_addr() {
                Ok(addr) => match addr.as_socket() {
                    Some(addr) => format!(", laddr={addr}"),
                    None => unimplemented!("unix socket"),
                },
                Err(_) => "".to_string(),
            };
            Ok(format!(
                "<compio.Socket fd={fd:?}, family={family}, type={ty}, protocol={proto}{laddr}>"
            ))
        } else {
            Ok("<compio.Socket (closed)>".to_string())
        }
    }

    #[pyo3(signature = (address, /))]
    fn connect<'py>(
        slf: &Bound<'py, Self>,
        py: Python<'py>,
        address: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        let slf = slf.borrow().pyloop.bind(py).borrow();
        slf.spawn_py(py, async move {
            let (domain, inner) = Python::attach(|py| {
                let this = this.bind(py).borrow();
                this.inner().cloned().map(|inner| (this.domain, inner))
            })?;
            match domain {
                Domain::IPV4 => {
                    let (result, port) = Python::attach(|py| {
                        let (host, port): (Bound<PyAny>, u16) = address.extract(py)?;
                        idna_converter(&host, |name| {
                            // https://github.com/rust-lang/rust/issues/101035
                            match str::from_utf8(name)?.parse::<Ipv4Addr>() {
                                Ok(ip) => Ok((Ok(ip), port)),
                                Err(_) => Ok((Err(name.to_owned()), port)),
                            }
                        })
                    })?;
                    let ip = match result {
                        Ok(ip) => ip,
                        Err(name) => name_to_ip(name, domain).await?.parse()?,
                    };
                    if cfg!(windows) && !Python::attach(|py| this.bind(py).borrow().bound) {
                        inner
                            .socket
                            .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())?;
                        Python::attach(|py| this.bind(py).borrow_mut().bound = true);
                    }
                    let addr = SocketAddr::new(ip.into(), port).into();
                    inner.connect_async(&addr).await?;
                }
                _ => unimplemented!(),
            };
            Python::attach(|py| {
                this.bind(py).borrow_mut().bound = true;
                py.None().into_py_any(py)
            })
        })
    }

    #[pyo3(signature = (bufsize, flags = 0, /))]
    fn recv<'py>(
        slf: &Bound<Self>,
        py: Python<'py>,
        bufsize: usize,
        flags: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        let slf = slf.borrow().pyloop.bind(py).borrow();
        slf.spawn_py(py, async move {
            let inner = Python::attach(|py| this.bind(py).borrow().inner().cloned())?;
            let buf: Vec<u8> = Vec::with_capacity(bufsize);
            let flags = op::RecvFlags::from_bits_retain(flags);
            let (bytes_read, buf) = buf_try!(@try inner.recv(buf, flags).await);
            Python::attach(|py| {
                PyBytes::new_with_writer(py, bytes_read, |w| Ok(w.write_all(&buf[..bytes_read])?))?
                    .into_py_any(py)
            })
        })
    }

    #[pyo3(signature = (data, flags = 0, /))]
    fn send<'py>(
        slf: &Bound<Self>,
        py: Python<'py>,
        data: Py<PyAny>,
        flags: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        let slf = slf.borrow().pyloop.bind(py).borrow();
        slf.spawn_py(py, async move {
            let (inner, buf) = Python::attach(|py| {
                let inner = this.bind(py).borrow().inner()?.clone();
                py_any_to_buffer(py, data.bind(py)).map(|buf| (inner, buf))
            })?;
            let flags = op::SendFlags::from_bits_retain(flags);
            let (bytes_written, _) = buf_try!(@try inner.send(buf, flags).await);
            drop(data);
            Python::attach(|py| bytes_written.into_py_any(py))
        })
    }

    #[pyo3(signature = (sslcontext=None, *, server_side=false, server_hostname=None))]
    fn start_tls<'py>(
        slf: &Bound<Self>,
        py: Python<'py>,
        sslcontext: Option<Py<PyAny>>,
        server_side: bool,
        server_hostname: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        let slf = slf.borrow().pyloop.bind(py).borrow();
        slf.spawn_py(py, async move {
            let mut metadata = SSLSocketMetadata::default();
            metadata.server_side = server_side;

            // First, verify parameters and prepare either TlsAcceptor or TlsConnector.
            let tls = Python::attach(|py| {
                let has_ossl = py_dynamic_openssl::load_py(py)?;
                // Coerce sslcontext to either SSLContext or RustlsContext
                let ctx: Either<_, Bound<RustlsContext>> = match sslcontext
                    .map(|ctx| ctx.into_bound(py))
                {
                    Some(ctx) if import::ssl::is_ssl_context(py, &ctx)? => has_ossl
                        .then(|| Either::Left(ctx))
                        .ok_or_else(|| PyTypeError::new_err("ssl.SSLContext is not supported"))?,
                    Some(ctx) => ctx.cast_into().map(Either::Right).map_err(|e| {
                        PyTypeError::new_err(format!("illegal sslcontext: {:?}", e.into_inner()))
                    })?,
                    None if !server_side => Either::Left(import::ssl::create_default_context(py)?),
                    None => Err(PyValueError::new_err("server_side requires sslcontext"))?,
                };
                // Build TlsAcceptor or TlsConnector from the context
                match ctx {
                    Either::Left(ctx) if has_ossl => SSLContext::try_from(ctx).map(|ctx| {
                        if server_side {
                            Either::Left(TlsAcceptor::from(ctx))
                        } else {
                            Either::Right(TlsConnector::from(ctx))
                        }
                    }),
                    Either::Left(ctx) => {
                        // This case is guaranteed to be a default client-side SSLContext
                        debug_assert!(!server_side);
                        metadata.implementation = SSLImpl::Rustls;
                        let ca_certs: Bound<PyList> =
                            ctx.call_method1("get_ca_certs", (true,))?.cast_into()?;
                        let mut root_store = rustls::RootCertStore::empty();
                        for cert in ca_certs.iter() {
                            let cert: Bound<PyBytes> = cert.cast_into()?;
                            let cert = cert.as_bytes();
                            root_store
                                .add(CertificateDer::from(cert))
                                .map_err(|e| PyValueError::new_err(e.to_string()))?;
                        }
                        let config = rustls::ClientConfig::builder()
                            .with_root_certificates(root_store)
                            .with_no_client_auth();
                        Ok(Either::Right(TlsConnector::from(Arc::new(config))))
                    }
                    Either::Right(ctx) => {
                        metadata.implementation = SSLImpl::Rustls;
                        let ctx = ctx.borrow();
                        Ok(match ctx.build(py, server_side)? {
                            Either::Left(c) => Either::Left(TlsAcceptor::from(c)),
                            Either::Right(c) => Either::Right(TlsConnector::from(c)),
                        })
                    }
                }
            })?;

            // Then, do TLS handshake accordingly
            let Some(inner) = Python::attach(|py| this.bind(py).borrow_mut().inner.take()) else {
                return Err(PyOSError::new_err("socket is closed"));
            };
            metadata.fd = inner.as_raw_fd();
            let stream = SocketStream { inner };
            let stream = match tls {
                Either::Left(acceptor) => extract_py_err(acceptor.accept(stream).await)?,
                Either::Right(connector) => {
                    let name = match server_hostname {
                        Some(name) => name,
                        None => stream
                            .inner
                            .socket
                            .peer_addr()?
                            .as_socket()
                            .ok_or_else(|| PyValueError::new_err("unknown server_hostname"))?
                            .ip()
                            .to_string(),
                    };
                    extract_py_err(connector.connect(&name, stream).await)?
                }
            };

            // At last, wrap the TlsStream in an SSLSocket
            Python::attach(|py| {
                let this = this.bind(py).borrow();
                SSLSocket::new(py, &this.pyloop, stream, metadata)?.into_py_any(py)
            })
        })
    }

    #[pyo3(signature = (how, /))]
    fn shutdown<'py>(slf: &Bound<Self>, py: Python<'py>, how: i32) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        let slf = slf.borrow().pyloop.bind(py).borrow();
        slf.spawn_py(py, async move {
            let (inner, how) = Python::attach(|py| {
                let inner = this.bind(py).borrow().inner()?.clone();
                let how = if how == import::socket::shut_wr(py)? {
                    Shutdown::Write
                } else if how == import::socket::shut_rd(py)? {
                    Shutdown::Read
                } else if how == import::socket::shut_rdwr(py)? {
                    Shutdown::Both
                } else {
                    return Err(PyValueError::new_err(format!(
                        "invalid shutdown how: {how}"
                    )));
                };
                Ok((inner, how))
            })?;
            inner.shutdown(how).await?;
            Python::attach(|py| py.None().into_py_any(py))
        })
    }

    fn close<'py>(slf: &Bound<'py, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        let slf = slf.borrow().pyloop.bind(py).borrow();
        slf.spawn_py(py, async move {
            if let Some(inner) = Python::attach(|py| this.bind(py).borrow_mut().inner.take()) {
                inner.close().await?;
            };
            Python::attach(|py| py.None().into_py_any(py))
        })
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySocket>()?;
    Ok(())
}
