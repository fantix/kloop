// This module is mostly copied and modified from:
// https://github.com/rust-openssl/rust-openssl/blob/openssl-v0.10.75/openssl/src/ssl/mod.rs
//
// SPDX-License-Identifier: Apache-2.0
// Copyright 2011-2017 Google Inc.
//           2013 Jack Lloyd
//           2013-2014 Steven Fackler
//
// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2026 Fantix King

use std::{
    cell::UnsafeCell,
    ffi::{CString, c_int, c_uchar, c_uint},
    fmt,
    io::{self, Read, Write},
    marker::PhantomData,
    mem::ManuallyDrop,
    net::IpAddr,
    panic::resume_unwind,
    ptr,
};

use self::error::InnerError;
pub use self::error::{Error, ErrorCode, HandshakeError};
use crate::{
    bio::{self, BioMethod, cvt, cvt_p},
    error::ErrorStack,
    sys as ffi,
};

pub struct Ssl(*mut ffi::SSL);

impl Drop for Ssl {
    fn drop(&mut self) {
        let ossl = crate::get();
        unsafe {
            (ossl.SSL_free)(self.0);
        }
    }
}

impl Ssl {
    pub fn new(ctx: *mut ffi::SSL_CTX) -> Result<Ssl, ErrorStack> {
        let ossl = crate::get();
        cvt_p(unsafe { (ossl.SSL_new)(ctx) }).map(Self)
    }

    pub fn connect<S>(self, stream: S) -> Result<SslStream<S>, HandshakeError<S>>
    where
        S: Read + Write,
    {
        let mut stream = SslStream::new(self, stream)?;
        match stream.connect() {
            Ok(()) => Ok(stream),
            Err(error) => match error.code() {
                ErrorCode::WANT_READ | ErrorCode::WANT_WRITE => {
                    Err(HandshakeError::WouldBlock(MidHandshakeSslStream {
                        stream,
                        error,
                    }))
                }
                _ => Err(HandshakeError::Failure(MidHandshakeSslStream {
                    stream,
                    error,
                })),
            },
        }
    }

    pub fn accept<S>(self, stream: S) -> Result<SslStream<S>, HandshakeError<S>>
    where
        S: Read + Write,
    {
        let mut stream = SslStream::new(self, stream)?;
        match stream.accept() {
            Ok(()) => Ok(stream),
            Err(error) => match error.code() {
                ErrorCode::WANT_READ | ErrorCode::WANT_WRITE => {
                    Err(HandshakeError::WouldBlock(MidHandshakeSslStream {
                        stream,
                        error,
                    }))
                }
                _ => Err(HandshakeError::Failure(MidHandshakeSslStream {
                    stream,
                    error,
                })),
            },
        }
    }

    fn get_raw_rbio(&self) -> *mut ffi::BIO {
        let ffi = crate::get();
        unsafe { (ffi.SSL_get_rbio)(self.0) }
    }

    fn get_error(&self, ret: c_int) -> ErrorCode {
        let ffi = crate::get();
        unsafe { ErrorCode::from_raw((ffi.SSL_get_error)(self.0, ret)) }
    }

    pub fn set_hostname(&mut self, hostname: &str) -> Result<(), ErrorStack> {
        let ffi = crate::get();
        let cstr = CString::new(hostname).unwrap();
        unsafe {
            cvt(ffi.SSL_set_tlsext_host_name(self.0, cstr.as_ptr() as *mut _) as c_int).map(|_| ())
        }
    }

    pub fn selected_alpn_protocol(&self) -> Option<&[u8]> {
        let ffi = crate::get();
        unsafe {
            let mut data: *const c_uchar = ptr::null();
            let mut len: c_uint = 0;
            (ffi.SSL_get0_alpn_selected)(self.0, &mut data, &mut len);

            if data.is_null() {
                None
            } else {
                Some(bio::from_raw_parts(data, len as usize))
            }
        }
    }

    pub fn param_mut(&mut self) -> &mut X509VerifyParamRef {
        let ffi = crate::get();
        unsafe { X509VerifyParamRef::from_ptr_mut((ffi.SSL_get0_param)(self.0)) }
    }
}

pub struct MidHandshakeSslStream<S> {
    stream: SslStream<S>,
    error: Error,
}

impl<S> MidHandshakeSslStream<S> {
    pub fn get_mut(&mut self) -> &mut S {
        self.stream.get_mut()
    }

    pub fn into_error(self) -> Error {
        self.error
    }
}

impl<S> MidHandshakeSslStream<S>
where
    S: Read + Write,
{
    pub fn handshake(mut self) -> Result<SslStream<S>, HandshakeError<S>> {
        match self.stream.do_handshake() {
            Ok(()) => Ok(self.stream),
            Err(error) => {
                self.error = error;
                match self.error.code() {
                    ErrorCode::WANT_READ | ErrorCode::WANT_WRITE => {
                        Err(HandshakeError::WouldBlock(self))
                    }
                    _ => Err(HandshakeError::Failure(self)),
                }
            }
        }
    }
}

pub struct SslStream<S> {
    ssl: ManuallyDrop<Ssl>,
    method: ManuallyDrop<BioMethod>,
    _p: PhantomData<S>,
}

impl<S> Drop for SslStream<S> {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.ssl);
            ManuallyDrop::drop(&mut self.method);
        }
    }
}

impl<S> fmt::Debug for SslStream<S>
where
    S: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("SslStream")
            .field("stream", &self.get_ref())
            .field("ssl", &self.ssl.0)
            .finish()
    }
}

impl<S: Read + Write> SslStream<S> {
    pub fn new(ssl: Ssl, stream: S) -> Result<Self, ErrorStack> {
        let ffi = crate::get();
        let (bio, method) = bio::new(stream)?;
        unsafe {
            (ffi.SSL_set_bio)(ssl.0, bio, bio);
        }

        Ok(Self {
            ssl: ManuallyDrop::new(ssl),
            method: ManuallyDrop::new(method),
            _p: PhantomData,
        })
    }

    pub fn connect(&mut self) -> Result<(), Error> {
        let ffi = crate::get();
        let ret = unsafe { (ffi.SSL_connect)(self.ssl.0) };
        if ret > 0 {
            Ok(())
        } else {
            Err(self.make_error(ret))
        }
    }

    pub fn accept(&mut self) -> Result<(), Error> {
        let ffi = crate::get();
        let ret = unsafe { (ffi.SSL_accept)(self.ssl.0) };
        if ret > 0 {
            Ok(())
        } else {
            Err(self.make_error(ret))
        }
    }

    pub fn do_handshake(&mut self) -> Result<(), Error> {
        let ffi = crate::get();
        let ret = unsafe { (ffi.SSL_do_handshake)(self.ssl.0) };
        if ret > 0 {
            Ok(())
        } else {
            Err(self.make_error(ret))
        }
    }

    pub fn ssl_read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        let ffi = crate::get();
        let mut readbytes = 0;
        let ret = unsafe {
            (ffi.SSL_read_ex)(
                self.ssl.0,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut readbytes,
            )
        };

        if ret > 0 {
            Ok(readbytes)
        } else {
            Err(self.make_error(ret))
        }
    }

    pub fn ssl_write(&mut self, buf: &[u8]) -> Result<usize, Error> {
        let ffi = crate::get();
        let mut written = 0;
        let ret =
            unsafe { (ffi.SSL_write_ex)(self.ssl.0, buf.as_ptr().cast(), buf.len(), &mut written) };

        if ret > 0 {
            Ok(written)
        } else {
            Err(self.make_error(ret))
        }
    }

    pub fn shutdown(&mut self) -> Result<ShutdownResult, Error> {
        let ffi = crate::get();
        match unsafe { (ffi.SSL_shutdown)(self.ssl.0) } {
            0 => Ok(ShutdownResult::Sent),
            1 => Ok(ShutdownResult::Received),
            n => Err(self.make_error(n)),
        }
    }
}

impl<S> SslStream<S> {
    fn make_error(&mut self, ret: c_int) -> Error {
        self.check_panic();

        let code = self.ssl.get_error(ret);

        let cause = match code {
            ErrorCode::SSL => Some(InnerError::Ssl(ErrorStack::get())),
            ErrorCode::SYSCALL => {
                let errs = ErrorStack::get();
                if errs.errors().is_empty() {
                    self.get_bio_error().map(InnerError::Io)
                } else {
                    Some(InnerError::Ssl(errs))
                }
            }
            ErrorCode::ZERO_RETURN => None,
            ErrorCode::WANT_READ | ErrorCode::WANT_WRITE => {
                self.get_bio_error().map(InnerError::Io)
            }
            _ => None,
        };

        Error { code, cause }
    }

    fn check_panic(&mut self) {
        if let Some(err) = unsafe { bio::take_panic::<S>(self.ssl.get_raw_rbio()) } {
            resume_unwind(err);
        }
    }

    fn get_bio_error(&mut self) -> Option<io::Error> {
        unsafe { bio::take_error::<S>(self.ssl.get_raw_rbio()) }
    }

    pub fn get_ref(&self) -> &S {
        unsafe {
            let bio = self.ssl.get_raw_rbio();
            bio::get_ref(bio)
        }
    }

    pub fn get_mut(&mut self) -> &mut S {
        unsafe {
            let bio = self.ssl.get_raw_rbio();
            bio::get_mut(bio)
        }
    }

    pub fn ssl(&self) -> &Ssl {
        &self.ssl
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShutdownResult {
    Sent,
    Received,
}

pub struct X509VerifyParamRef(UnsafeCell<()>);

impl X509VerifyParamRef {
    #[inline]
    unsafe fn from_ptr_mut<'a>(ptr: *mut ffi::X509_VERIFY_PARAM) -> &'a mut Self {
        unsafe { &mut *(ptr as *mut _) }
    }

    #[inline]
    fn as_ptr(&self) -> *mut ffi::X509_VERIFY_PARAM {
        self as *const _ as *mut _
    }

    pub fn set_host(&mut self, host: &str) -> Result<(), ErrorStack> {
        let ffi = crate::get();
        unsafe {
            let raw_host = if host.is_empty() { "\0" } else { host };
            cvt((ffi.X509_VERIFY_PARAM_set1_host)(
                self.as_ptr(),
                raw_host.as_ptr() as *const _,
                isize::try_from(host.len()).expect("host length <= isize::MAX"),
            ))
            .map(|_| ())
        }
    }

    pub fn set_ip(&mut self, ip: IpAddr) -> Result<(), ErrorStack> {
        let ffi = crate::get();
        unsafe {
            let mut buf = [0; 16];
            let len = match ip {
                IpAddr::V4(addr) => {
                    buf[..4].copy_from_slice(&addr.octets());
                    4
                }
                IpAddr::V6(addr) => {
                    buf.copy_from_slice(&addr.octets());
                    16
                }
            };
            cvt((ffi.X509_VERIFY_PARAM_set1_ip)(
                self.as_ptr(),
                buf.as_ptr() as *const _,
                len,
            ))
            .map(|_| ())
        }
    }
}

mod error {
    use std::{error, ffi::c_int, fmt, io};

    use crate::{error::ErrorStack, ssl::MidHandshakeSslStream, sys as ffi};

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub struct ErrorCode(c_int);

    impl ErrorCode {
        pub const SSL: ErrorCode = ErrorCode(ffi::SSL_ERROR_SSL);
        pub const SYSCALL: ErrorCode = ErrorCode(ffi::SSL_ERROR_SYSCALL);
        pub const WANT_CLIENT_HELLO_CB: ErrorCode = ErrorCode(ffi::SSL_ERROR_WANT_CLIENT_HELLO_CB);
        pub const WANT_READ: ErrorCode = ErrorCode(ffi::SSL_ERROR_WANT_READ);
        pub const WANT_WRITE: ErrorCode = ErrorCode(ffi::SSL_ERROR_WANT_WRITE);
        pub const ZERO_RETURN: ErrorCode = ErrorCode(ffi::SSL_ERROR_ZERO_RETURN);

        pub fn from_raw(raw: c_int) -> ErrorCode {
            ErrorCode(raw)
        }
    }

    #[derive(Debug)]
    pub(crate) enum InnerError {
        Io(io::Error),
        Ssl(ErrorStack),
    }

    #[derive(Debug)]
    pub struct Error {
        pub(crate) code: ErrorCode,
        pub(crate) cause: Option<InnerError>,
    }

    impl Error {
        pub fn code(&self) -> ErrorCode {
            self.code
        }

        pub fn io_error(&self) -> Option<&io::Error> {
            match self.cause {
                Some(InnerError::Io(ref e)) => Some(e),
                _ => None,
            }
        }

        pub fn into_io_error(self) -> Result<io::Error, Error> {
            match self.cause {
                Some(InnerError::Io(e)) => Ok(e),
                _ => Err(self),
            }
        }

        pub fn ssl_error(&self) -> Option<&ErrorStack> {
            match self.cause {
                Some(InnerError::Ssl(ref e)) => Some(e),
                _ => None,
            }
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.code {
                ErrorCode::ZERO_RETURN => fmt.write_str("the SSL session has been shut down"),
                ErrorCode::WANT_READ => match self.io_error() {
                    Some(_) => fmt.write_str("a nonblocking read call would have blocked"),
                    None => fmt.write_str("the operation should be retried"),
                },
                ErrorCode::WANT_WRITE => match self.io_error() {
                    Some(_) => fmt.write_str("a nonblocking write call would have blocked"),
                    None => fmt.write_str("the operation should be retried"),
                },
                ErrorCode::SYSCALL => match self.io_error() {
                    Some(err) => write!(fmt, "{}", err),
                    None => fmt.write_str("unexpected EOF"),
                },
                ErrorCode::SSL => match self.ssl_error() {
                    Some(e) => write!(fmt, "{}", e),
                    None => fmt.write_str("OpenSSL error"),
                },
                ErrorCode(code) => write!(fmt, "unknown error code {}", code),
            }
        }
    }

    impl error::Error for Error {
        fn source(&self) -> Option<&(dyn error::Error + 'static)> {
            match self.cause {
                Some(InnerError::Io(ref e)) => Some(e),
                Some(InnerError::Ssl(ref e)) => Some(e),
                None => None,
            }
        }
    }

    pub enum HandshakeError<S> {
        SetupFailure(ErrorStack),
        Failure(MidHandshakeSslStream<S>),
        WouldBlock(MidHandshakeSslStream<S>),
    }

    impl<S> From<ErrorStack> for HandshakeError<S> {
        fn from(e: ErrorStack) -> HandshakeError<S> {
            HandshakeError::SetupFailure(e)
        }
    }
}
