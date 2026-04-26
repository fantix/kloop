// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2026 Fantix King

use std::{io, net::Shutdown};

use compio::{
    buf::{BufResult, IntoInner, IoBuf, IoBufMut, IoVectoredBuf, IoVectoredBufMut, buf_try},
    driver::{
        ToSharedFd, impl_raw_fd,
        op::{self, BufResultExt, RecvResultExt, VecBufResultExt},
    },
    io::{
        AsyncRead, AsyncWrite,
        ancillary::{AsyncReadAncillary, AsyncWriteAncillary},
        util::Splittable,
    },
};
use pyo3::{
    exceptions::PyTypeError,
    prelude::*,
    types::{PyByteArray, PyBytes, PyList, PyTuple},
};
use socket2::{Domain, Protocol, SockAddr, Socket as Socket2, Type};

pub use self::socket::PySocket;
use crate::{
    import,
    runtime::{self, Attacher},
};

mod socket;
mod ssl;

#[derive(Debug, Clone)]
pub struct SocketStream {
    inner: Socket,
}

impl AsyncRead for &SocketStream {
    #[inline]
    async fn read<B: IoBufMut>(&mut self, buf: B) -> BufResult<usize, B> {
        self.inner.recv(buf, op::RecvFlags::empty()).await
    }
}

impl AsyncReadAncillary for &SocketStream {
    #[inline]
    async fn read_with_ancillary<T: IoBufMut, C: IoBufMut>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<(usize, usize), (T, C)> {
        self.inner
            .recv_msg(buffer, control, op::RecvFlags::empty())
            .await
            .map_res(|(res, len, _addr)| (res, len))
    }

    #[inline]
    async fn read_vectored_with_ancillary<T: IoVectoredBufMut, C: IoBufMut>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<(usize, usize), (T, C)> {
        self.inner
            .recv_msg_vectored(buffer, control, op::RecvFlags::empty())
            .await
            .map_res(|(res, len, _addr)| (res, len))
    }
}

impl AsyncWrite for &SocketStream {
    async fn write<T: IoBuf>(&mut self, buf: T) -> BufResult<usize, T> {
        self.inner.send(buf, op::SendFlags::empty()).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.inner.shutdown(Shutdown::Write).await
    }
}

impl AsyncWriteAncillary for &SocketStream {
    #[inline]
    async fn write_with_ancillary<T: IoBuf, C: IoBuf>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<usize, (T, C)> {
        self.inner
            .send_msg(buffer, control, None, op::SendFlags::empty())
            .await
    }

    #[inline]
    async fn write_vectored_with_ancillary<T: IoVectoredBuf, C: IoBuf>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<usize, (T, C)> {
        self.inner
            .send_msg_vectored(buffer, control, None, op::SendFlags::empty())
            .await
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

impl AsyncReadAncillary for SocketStream {
    async fn read_with_ancillary<T: IoBufMut, C: IoBufMut>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<(usize, usize), (T, C)> {
        (&*self).read_with_ancillary(buffer, control).await
    }

    async fn read_vectored_with_ancillary<T: IoVectoredBufMut, C: IoBufMut>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<(usize, usize), (T, C)> {
        (&*self).read_vectored_with_ancillary(buffer, control).await
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

impl AsyncWriteAncillary for SocketStream {
    async fn write_with_ancillary<T: IoBuf, C: IoBuf>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<usize, (T, C)> {
        (&*self).write_with_ancillary(buffer, control).await
    }

    async fn write_vectored_with_ancillary<T: IoVectoredBuf, C: IoBuf>(
        &mut self,
        buffer: T,
        control: C,
    ) -> BufResult<usize, (T, C)> {
        (&*self)
            .write_vectored_with_ancillary(buffer, control)
            .await
    }
}

pub struct ReadHalf(Socket);

pub struct WriteHalf(Socket);

impl AsyncRead for ReadHalf {
    #[inline]
    async fn read<B: IoBufMut>(&mut self, buf: B) -> BufResult<usize, B> {
        self.0.recv(buf, op::RecvFlags::empty()).await
    }
}

impl AsyncWrite for WriteHalf {
    async fn write<T: IoBuf>(&mut self, buf: T) -> BufResult<usize, T> {
        self.0.send(buf, op::SendFlags::empty()).await
    }

    async fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        self.0.shutdown(Shutdown::Write).await
    }
}

impl Splittable for SocketStream {
    type ReadHalf = ReadHalf;
    type WriteHalf = WriteHalf;

    fn split(self) -> (Self::ReadHalf, Self::WriteHalf) {
        (ReadHalf(self.inner.clone()), WriteHalf(self.inner))
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
    socket::register(m)?;
    ssl::register(m)?;
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
        let op = op::Connect::new(self.to_shared_fd(), addr.clone());
        let (_, _op) = buf_try!(@try runtime::execute(op).await);
        #[cfg(windows)]
        _op.update_context()?;
        Ok(())
    }

    async fn recv<B: IoBufMut>(&self, buffer: B, flags: op::RecvFlags) -> BufResult<usize, B> {
        let fd = self.to_shared_fd();
        let op = op::Recv::new(fd, buffer, flags);
        let res = runtime::execute(op).await.into_inner();
        unsafe { res.map_advanced() }
    }

    pub async fn recv_msg<T: IoBufMut, C: IoBufMut>(
        &self,
        buffer: T,
        control: C,
        flags: op::RecvFlags,
    ) -> BufResult<(usize, usize, Option<SockAddr>), (T, C)> {
        self.recv_msg_vectored([buffer], control, flags)
            .await
            .map_buffer(|([buffer], control)| (buffer, control))
    }

    pub async fn recv_msg_vectored<T: IoVectoredBufMut, C: IoBufMut>(
        &self,
        buffer: T,
        control: C,
        flags: op::RecvFlags,
    ) -> BufResult<(usize, usize, Option<SockAddr>), (T, C)> {
        let fd = self.to_shared_fd();
        let op = op::RecvMsg::new(fd, buffer, control, flags);
        let res = runtime::execute(op).await;
        let res = res.into_inner().map_addr();
        unsafe { res.map_vec_advanced() }
    }

    async fn send<T: IoBuf>(&self, buffer: T, flags: op::SendFlags) -> BufResult<usize, T> {
        let fd = self.to_shared_fd();
        let op = op::Send::new(fd, buffer, flags);
        runtime::execute(op).await.into_inner()
    }

    pub async fn send_msg<T: IoBuf, C: IoBuf>(
        &self,
        buffer: T,
        control: C,
        addr: Option<&SockAddr>,
        flags: op::SendFlags,
    ) -> BufResult<usize, (T, C)> {
        self.send_msg_vectored([buffer], control, addr, flags)
            .await
            .map_buffer(|([buffer], control)| (buffer, control))
    }

    pub async fn send_msg_vectored<T: IoVectoredBuf, C: IoBuf>(
        &self,
        buffer: T,
        control: C,
        addr: Option<&SockAddr>,
        flags: op::SendFlags,
    ) -> BufResult<usize, (T, C)> {
        let fd = self.to_shared_fd();
        let op = op::SendMsg::new(fd, buffer, control, addr.cloned(), flags);
        runtime::execute(op).await.into_inner()
    }

    async fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        #[cfg(unix)]
        {
            let fd = self.to_shared_fd();
            let op = op::ShutdownSocket::new(fd, how);
            runtime::execute(op).await.0?;
            Ok(())
        }
        #[cfg(windows)]
        self.socket.shutdown(how)
    }

    async fn close(self) -> io::Result<()> {
        let fd = self.socket.into_inner().take().await;
        if let Some(fd) = fd {
            let op = op::CloseSocket::new(fd.into());
            runtime::execute(op).await.0?;
        }
        Ok(())
    }
}

impl_raw_fd!(Socket, Socket2, socket, socket);
