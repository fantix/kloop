// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2025 Fantix King

use std::io;

use compio::{buf::IoBuf, tls::rustls};
use pyo3::{buffer::PyBuffer, prelude::*};

mod event_loop;
mod handle;
mod import;
mod net;
mod owned;
mod runtime;
mod send_wrapper;
mod thread;

/// A Python module implemented in Rust. The name of this module must match
/// the `lib.name` setting in the `Cargo.toml`, else Python will not be able to
/// import the module.
#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    #[cfg(feature = "enable_log")]
    {
        use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(true)
                    .with_level(true),
            )
            .with(EnvFilter::from_default_env())
            .try_init();
    }

    event_loop::register(m)?;
    handle::register(m)?;
    net::register(m)?;
    Ok(())
}

fn py_any_to_buffer(py: Python, data: &Bound<PyAny>) -> PyResult<Either<&'static [u8], Vec<u8>>> {
    let pybuf: PyBuffer<u8> = PyBuffer::get(data)?;
    if pybuf.is_c_contiguous() {
        let ptr = pybuf.buf_ptr() as *mut u8;
        let len = pybuf.len_bytes();
        Ok(Either::Left(unsafe {
            std::slice::from_raw_parts(ptr, len)
        }))
    } else {
        pybuf.to_vec(py).map(Either::Right)
    }
}

#[derive(Clone)]
enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L, R> Either<L, R> {
    fn into<T: From<L> + From<R>>(self) -> T {
        match self {
            Self::Left(l) => T::from(l),
            Self::Right(r) => T::from(r),
        }
    }
}

impl<L: IoBuf, R: IoBuf> IoBuf for Either<L, R> {
    fn as_init(&self) -> &[u8] {
        match self {
            Either::Left(l) => l.as_init(),
            Either::Right(r) => r.as_init(),
        }
    }
}

impl<T: ?Sized, L: AsRef<T>, R: AsRef<T>> AsRef<T> for Either<L, R> {
    fn as_ref(&self) -> &T {
        match self {
            Either::Left(l) => l.as_ref(),
            Either::Right(r) => r.as_ref(),
        }
    }
}

fn extract_py_err<T>(res: io::Result<T>) -> PyResult<T> {
    match res {
        Ok(val) => Ok(val),
        Err(io_err) => match io_err.downcast::<PyErr>() {
            Ok(e) => Err(e),
            Err(io_err) => match io_err.downcast::<rustls::Error>() {
                Ok(rustls::Error::Other(other_err)) => match other_err.0.downcast_ref::<PyErr>() {
                    Some(py_err) => Err(Python::attach(|py| py_err.clone_ref(py))),
                    None => Err(io::Error::other(rustls::Error::Other(other_err)).into()),
                },
                Ok(rustls_err) => Err(io::Error::other(rustls_err).into()),
                Err(e) => Err(e.into()),
            },
        },
    }
}
