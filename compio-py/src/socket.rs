// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2026 Fantix King

use std::{
    io,
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4},
    sync::Arc,
};

use compio::{
    buf::{BufResult, IntoInner, IoBuf, IoBufMut, IoVectoredBufMut, buf_try},
    driver::{
        AsRawFd, ToSharedFd, impl_raw_fd,
        op::{BufResultExt, CloseSocket, Connect, Recv, Send, ShutdownSocket},
    },
    io::{AsyncRead, AsyncWrite},
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
    types::{PyByteArray, PyBytes, PyList, PyTuple},
};
use socket2::{Domain, Protocol, SockAddr, Socket as Socket2, Type};

use crate::{
    Either,
    event_loop::CompioLoop,
    extract_py_err, import, py_any_to_buffer,
    runtime::{self, Attacher},
    ssl::{RustlsContext, SSLImpl, SSLSocket, SSLSocketMetadata},
};

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
    fn connect<'py>(&mut self, py: Python<'py>, address: Py<PyAny>) -> PyResult<Bound<'py, PyAny>> {
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            let Some(inner) = &self.inner else {
                return Err(PyOSError::new_err("socket is closed"));
            };
            match self.domain {
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
                        Err(name) => name_to_ip(name, self.domain).await?.parse()?,
                    };
                    if cfg!(windows) && !self.bound {
                        inner
                            .socket
                            .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into())?;
                        self.bound = true;
                    }
                    let addr = SocketAddr::new(ip.into(), port).into();
                    inner.connect_async(&addr).await?;
                    self.bound = true;
                }
                _ => unimplemented!(),
            };
            Python::attach(|py| py.None().into_py_any(py))
        })
    }

    #[pyo3(signature = (bufsize, flags = 0, /))]
    fn recv<'py>(
        &self,
        py: Python<'py>,
        bufsize: usize,
        flags: i32,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            let inner = self.inner()?;
            let buf: Vec<u8> = Vec::with_capacity(bufsize);
            let (bytes_read, buf) = buf_try!(@try inner.recv(buf, flags).await);
            Python::attach(|py| {
                PyBytes::new_with_writer(py, bytes_read, |w| Ok(w.write_all(&buf[..bytes_read])?))?
                    .into_py_any(py)
            })
        })
    }

    #[pyo3(signature = (data, flags = 0, /))]
    fn send<'py>(
        &self,
        py: Python<'py>,
        data: Py<PyAny>,
        flags: i32,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            let inner = self.inner()?;
            let buf = Python::attach(|py| py_any_to_buffer(py, data.bind(py)))?;
            let (bytes_written, _) = buf_try!(@try inner.send(buf, flags).await);
            drop(data);
            Python::attach(|py| bytes_written.into_py_any(py))
        })
    }

    #[pyo3(signature = (sslcontext=None, *, server_side=false, server_hostname=None))]
    fn start_tls<'py>(
        &mut self,
        py: Python<'py>,
        sslcontext: Option<Py<PyAny>>,
        server_side: bool,
        server_hostname: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
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
            let Some(inner) = self.inner.take() else {
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
            Python::attach(|py| SSLSocket::new(py, &self.pyloop, stream, metadata)?.into_py_any(py))
        })
    }

    #[pyo3(signature = (how, /))]
    fn shutdown<'py>(&self, py: Python<'py>, how: i32) -> PyResult<Bound<'py, PyAny>> {
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            let inner = self.inner()?;
            let how = Python::attach(|py| {
                if how == import::socket::shut_wr(py)? {
                    Ok(Shutdown::Write)
                } else if how == import::socket::shut_rd(py)? {
                    Ok(Shutdown::Read)
                } else if how == import::socket::shut_rdwr(py)? {
                    Ok(Shutdown::Both)
                } else {
                    return Err(PyValueError::new_err(format!(
                        "invalid shutdown how: {how}"
                    )));
                }
            })?;
            inner.shutdown(how).await?;
            Python::attach(|py| py.None().into_py_any(py))
        })
    }

    fn close<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.pyloop.bind(py).borrow().spawn_py(py, async move {
            if let Some(inner) = self.inner.take() {
                inner.close().await?;
            };
            Python::attach(|py| py.None().into_py_any(py))
        })
    }
}

#[derive(Debug, Clone)]
pub struct SocketStream {
    inner: Socket,
}

impl AsyncRead for &SocketStream {
    #[inline]
    async fn read<B: IoBufMut>(&mut self, buf: B) -> BufResult<usize, B> {
        self.inner.recv(buf, 0).await
    }
}

impl AsyncWrite for &SocketStream {
    async fn write<T: IoBuf>(&mut self, buf: T) -> BufResult<usize, T> {
        self.inner.send(buf, 0).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown(Shutdown::Write).await
    }
}

impl AsyncRead for SocketStream {
    #[inline]
    async fn read<B: IoBufMut>(&mut self, buf: B) -> BufResult<usize, B> {
        (&*self).read(buf).await
    }

    #[inline]
    async fn read_vectored<V: IoVectoredBufMut>(&mut self, buf: V) -> BufResult<usize, V> {
        (&*self).read_vectored(buf).await
    }
}

impl AsyncWrite for SocketStream {
    async fn write<T: IoBuf>(&mut self, buf: T) -> BufResult<usize, T> {
        (&*self).write(buf).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        (&*self).flush().await
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        (&*self).shutdown().await
    }
}

impl_raw_fd!(SocketStream, socket2::Socket, inner, socket);

async fn name_to_ip(name: Vec<u8>, family: impl Into<i32>) -> PyResult<String> {
    let family = family.into();
    runtime::asyncify(move || {
        Python::attach(|py| {
            let args = (name, py.None(), family);
            let result_list: Bound<PyList> =
                import::socket::getaddrinfo(py, args, None)?.cast_into()?;
            let result: Bound<PyTuple> = result_list.get_item(0)?.cast_into()?;
            let addr: Bound<PyTuple> = result.get_item(result.len() - 1)?.cast_into()?;
            addr.get_item(0)?.extract()
        })
    })
    .await
}

fn idna_converter<T, F>(obj: &Bound<PyAny>, f: F) -> PyResult<T>
where
    F: FnOnce(&[u8]) -> PyResult<T>,
{
    if let Ok(bytes) = obj.cast::<PyBytes>() {
        f(bytes.as_bytes())
    } else if let Ok(bytes) = obj.cast::<PyByteArray>() {
        f(unsafe { bytes.as_bytes() })
    } else if let Ok(str) = obj.extract::<&str>() {
        if str.is_ascii() {
            f(str.as_bytes())
        } else {
            f(obj
                .call_method1("encode", ("idna",))?
                .cast::<PyBytes>()?
                .as_bytes())
        }
    } else {
        Err(PyTypeError::new_err(format!(
            "str, bytes or bytearray expected, not {}",
            obj.get_type()
        )))
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySocket>()?;
    Ok(())
}

// Socket is copied and modified from compio-net:
// https://github.com/compio-rs/compio/blob/8cf1b4db78c93f37d462b85bd1f445caa7fca4d5/compio-net/src/socket.rs
// Copyright (c) 2023 Berrysoft

#[derive(Debug, Clone)]
struct Socket {
    socket: Attacher<Socket2>,
}

impl Socket {
    fn from_socket2(socket: Socket2) -> io::Result<Self> {
        let socket = Attacher::new(socket)?;
        Ok(Self { socket })
    }

    async fn new(domain: Domain, ty: Type, protocol: Option<Protocol>) -> io::Result<Self> {
        Self::from_socket2({
            #[cfg(windows)]
            {
                runtime::asyncify(move || Socket2::new(domain, ty, protocol)).await?
            }
            #[cfg(unix)]
            {
                use compio::driver::op::CreateSocket;

                let op = CreateSocket::new(
                    domain.into(),
                    ty.into(),
                    protocol.map(|p| p.into()).unwrap_or_default(),
                );
                let (_, op) = buf_try!(@try runtime::execute(op).await);
                op.into_inner()
            }
        })
    }

    async fn connect_async(&self, addr: &SockAddr) -> io::Result<()> {
        let op = Connect::new(self.to_shared_fd(), addr.clone());
        let (_, _op) = buf_try!(@try runtime::execute(op).await);
        #[cfg(windows)]
        _op.update_context()?;
        Ok(())
    }

    async fn recv<B: IoBufMut>(&self, buffer: B, flags: i32) -> BufResult<usize, B> {
        let fd = self.to_shared_fd();
        let op = Recv::new(fd, buffer, flags);
        let res = runtime::execute(op).await.into_inner();
        unsafe { res.map_advanced() }
    }

    async fn send<T: IoBuf>(&self, buffer: T, flags: i32) -> BufResult<usize, T> {
        let fd = self.to_shared_fd();
        let op = Send::new(fd, buffer, flags);
        runtime::execute(op).await.into_inner()
    }

    async fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let fd = self.to_shared_fd();
        let op = ShutdownSocket::new(fd, how);
        runtime::execute(op).await.0?;
        Ok(())
    }

    async fn close(self) -> io::Result<()> {
        let fd = self.socket.into_inner().take().await;
        if let Some(fd) = fd {
            let op = CloseSocket::new(fd.into());
            runtime::execute(op).await.0?;
        }
        Ok(())
    }
}

impl_raw_fd!(Socket, Socket2, socket, socket);
