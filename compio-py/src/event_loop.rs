// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2025 Fantix King

use std::sync::{
    Arc,
    atomic::{self, AtomicBool},
};

use compio::driver::{SharedFd, ToSharedFd, op};
use compio_executor::JoinHandle;
use compio_log::*;
use once_cell::sync::OnceCell;
use pyo3::{
    IntoPyObjectExt,
    buffer::PyBuffer,
    exceptions::{PyRuntimeError, PyValueError},
    ffi::c_str,
    prelude::*,
    types::{PyDict, PyMapping, PyTuple, PyWeakrefReference},
};

use crate::{
    handle::{Handle, TimerHandle},
    import, net,
    owned::{self, OwnedRefCell},
    runtime::{self, Runtime},
};

static COMPIO_FUTURE: OnceCell<Py<PyAny>> = OnceCell::new();

#[derive(Debug, Clone, Copy, Default)]
pub enum KtlsMode {
    #[default]
    Prefer,
    Require,
    Disabled,
}

impl KtlsMode {
    pub fn enabled(self) -> bool {
        matches!(self, KtlsMode::Prefer | KtlsMode::Require)
    }
}

#[pyclass(subclass)]
pub struct CompioLoop {
    runtime: OwnedRefCell<Runtime>,
    stopping: Arc<AtomicBool>,
    debug: Arc<AtomicBool>,
    registered: Py<PyMapping>,
    ktls_mode: KtlsMode,
}

#[pymethods]
impl CompioLoop {
    #[new]
    fn new(py: Python) -> PyResult<Self> {
        Ok(Self {
            runtime: Default::default(),
            stopping: Default::default(),
            debug: Default::default(),
            registered: import::weakref::weak_key_dict(py)?.cast_into()?.unbind(),
            ktls_mode: KtlsMode::default(),
        })
    }

    fn __init__(slf: &Bound<Self>, py: Python) -> PyResult<()> {
        debug!("Initializing CompioLoop");
        let stopping = slf.borrow().stopping.clone();
        let pyloop = PyWeakrefReference::new(slf)?.unbind();
        let slf = slf.clone().unbind();
        py.detach(|| {
            Runtime::new(pyloop, stopping).map(|runtime| {
                Python::attach(|py| {
                    slf.bind(py)
                        .borrow_mut()
                        .runtime
                        .init(runtime)
                        .expect("uninitialized");
                })
            })
        })?;
        debug!("CompioLoop initialized");
        Ok(())
    }

    fn is_running(&self) -> bool {
        match self.runtime.acquire(false) {
            Ok(_) => false,
            _ => true,
        }
    }

    fn is_closed(&self) -> bool {
        match self.runtime.get() {
            Ok(None) => true,
            _ => false,
        }
    }

    fn get_driver_type(&self) -> PyResult<String> {
        Ok(format!("{:?}", self.runtime()?.driver_type()))
    }

    #[pyo3(signature = (callback, *args, context=None))]
    fn call_soon<'py>(
        &self,
        py: Python<'py>,
        callback: Py<PyAny>,
        args: Py<PyTuple>,
        context: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, Handle>> {
        let runtime = self.runtime()?;
        Handle::new(py, callback, args, context)?.schedule_soon(&runtime, py)
    }

    #[pyo3(signature = (when, callback, *args, context=None))]
    fn call_at<'py>(
        &self,
        py: Python<'py>,
        when: f64,
        callback: Py<PyAny>,
        args: Py<PyTuple>,
        context: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, TimerHandle>> {
        let runtime = self.runtime()?;
        Handle::new(py, callback, args, context)?.schedule_at(&runtime, py, when)
    }

    #[pyo3(signature = (delay, callback, *args, context=None))]
    fn call_later<'py>(
        &self,
        py: Python<'py>,
        delay: f64,
        callback: Py<PyAny>,
        args: Py<PyTuple>,
        context: Option<Py<PyAny>>,
    ) -> PyResult<Bound<'py, TimerHandle>> {
        let runtime = self.runtime()?;
        let when = runtime.since_epoch().as_secs_f64() + delay;
        Handle::new(py, callback, args, context)?.schedule_at(&runtime, py, when)
    }

    fn sleep<'py>(&self, py: Python<'py>, delay: f64) -> PyResult<Bound<'py, PyAny>> {
        let runtime = self.runtime()?;
        let when = runtime.since_epoch().as_secs_f64() + delay;
        let timer = runtime.timer(when);
        self.spawn_py(py, async {
            timer.await;
            Ok(Python::attach(|py| py.None()))
        })
    }

    fn time(&self) -> PyResult<f64> {
        Ok(self.runtime()?.since_epoch().as_secs_f64())
    }

    fn run_forever(&self, py: Python) -> PyResult<()> {
        py.detach(|| {
            let _guard = self.runtime.acquire(false).map_err(|_| {
                PyErr::new::<PyRuntimeError, _>("This event loop is already running")
            })?;
            let _clear_stopping_guard = StoreFalseGuard(self.stopping.clone());
            self.runtime()?.run()
        })
    }

    fn stop(&self) {
        debug!("Stopping event loop");
        self.stopping.store(true, atomic::Ordering::SeqCst);
    }

    fn close(slf: &Bound<Self>) -> PyResult<()> {
        if let Some(runtime) = slf
            .try_borrow_mut()
            .map_err(|_| PyErr::new::<PyRuntimeError, _>("Cannot close a running event loop"))?
            .runtime
            .take()
        {
            drop(runtime);
        }
        Ok(())
    }

    fn get_debug(&self) -> bool {
        self.debug.load(atomic::Ordering::Acquire)
    }

    fn set_debug(&self, value: bool) {
        self.debug.store(value, atomic::Ordering::Release);
    }

    // Network I/O methods returning Futures.

    #[pyo3(signature = (*args, **kwargs))]
    fn getaddrinfo<'py>(
        &self,
        py: Python<'py>,
        args: Py<PyTuple>,
        kwargs: Option<Py<PyDict>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let f = || {
            Python::attach(|py| import::socket::getaddrinfo(py, args, kwargs).map(|r| r.unbind()))
        };
        self.spawn_py(py, runtime::asyncify(f))
    }

    #[pyo3(signature = (*args, **kwargs))]
    fn getnameinfo<'py>(
        &self,
        py: Python<'py>,
        args: Py<PyTuple>,
        kwargs: Option<Py<PyDict>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let f = || Python::attach(|py| import::socket::getnameinfo(py, args, kwargs));
        self.spawn_py(py, runtime::asyncify(f))
    }

    // Completion based I/O methods returning Futures.

    fn sock_recv_into<'py>(
        slf: &Bound<Self>,
        py: Python<'py>,
        sock: Py<PyAny>,
        buf: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let this = slf.clone().unbind();
        slf.borrow().spawn_py(py, async move {
            let op = Python::attach(|py| {
                let pybuf: PyBuffer<u8> = PyBuffer::get(buf.bind(py))?;
                if pybuf.readonly() {
                    return Err(PyErr::new::<PyValueError, _>(
                        "buffer argument must be writable",
                    ));
                }
                if !pybuf.is_c_contiguous() {
                    return Err(PyErr::new::<PyValueError, _>(
                        "buffer argument must be C-contiguous",
                    ));
                }
                let ptr = pybuf.buf_ptr() as *mut u8;
                let len = pybuf.len_bytes();
                let buf = unsafe { std::slice::from_raw_parts_mut(ptr, len) };

                let fd = this.bind(py).borrow().socket_to_fd(py, &sock)?;
                Ok(op::Recv::new(fd, buf, op::RecvFlags::empty()))
            })?;
            let nbytes = runtime::execute(op).await.0?;

            // make sure sock and buf are captured and not dropped too early
            drop(sock);
            drop(buf);

            Python::attach(|py| nbytes.into_py_any(py))
        })
    }

    #[pyo3(signature = (family=-1, r#type=-1, proto=-1))]
    fn create_socket<'py>(
        slf: Bound<'py, Self>,
        py: Python<'py>,
        family: i32,
        r#type: i32,
        proto: i32,
    ) -> PyResult<Bound<'py, PyAny>> {
        use socket2::{Domain, Protocol, Type};

        let domain = if family == -1 {
            Domain::IPV4
        } else {
            Domain::from(family)
        };
        let ty = if r#type == -1 {
            Type::STREAM
        } else {
            Type::from(r#type)
        };
        let protocol = if proto == -1 {
            None
        } else {
            Some(Protocol::from(proto))
        };
        let pyloop = slf.clone().unbind();
        slf.borrow()
            .spawn_py(py, net::PySocket::new(pyloop, domain, ty, protocol))
    }

    #[getter]
    fn ktls_mode(&self) -> String {
        format!("{:?}", self.ktls_mode)
    }

    #[setter]
    fn set_ktls_mode(&mut self, mode: &Bound<PyAny>) -> PyResult<()> {
        self.ktls_mode = match mode.str()?.to_str()? {
            "Prefer" => KtlsMode::Prefer,
            "Require" => KtlsMode::Require,
            "Disabled" => KtlsMode::Disabled,
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown KTLS mode: {}",
                    other
                )));
            }
        };
        Ok(())
    }
}

impl CompioLoop {
    #[inline]
    pub fn runtime(&self) -> PyResult<owned::Ref<'_, Runtime>> {
        match self.runtime.get() {
            Ok(Some(rv)) => Ok(rv),
            Ok(None) => Err(PyErr::new::<PyRuntimeError, _>("CompioLoop is closed")),
            Err(_) => Err(PyErr::new::<PyRuntimeError, _>(
                "Non-thread-safe operation invoked on an event loop other than the current one",
            )),
        }
    }

    /// Spawn a Rust Future onto the event loop, returning a connected Python
    /// Future. When the Python Future is cancelled, the Rust Future is also
    /// cancelled.
    pub fn spawn_py<'py, F>(&self, py: Python<'py>, fut: F) -> PyResult<Bound<'py, PyAny>>
    where
        F: Future<Output = PyResult<Py<PyAny>>> + 'static,
    {
        let cancellable = Bound::new(py, Cancellable { task: None })?;
        let rv = COMPIO_FUTURE
            .get()
            .ok_or_else(|| PyRuntimeError::new_err("CompioFuture is not ready"))?
            .bind(py)
            .call1((cancellable.clone(),))?;

        let py_fut: Py<PyAny> = rv.clone().unbind();
        let cancellable_py = cancellable.clone().unbind();
        let join_handle = self.runtime()?.spawn(async move {
            let result = fut.await;
            Python::attach(|py| {
                let py_fut = py_fut.bind(py);
                if let Err(e) = match &result {
                    Ok(result) => py_fut.call_method1("set_result", (result,)),
                    Err(e) => py_fut.call_method1("set_exception", (e,)),
                } {
                    if cancellable_py.bind(py).borrow().task.is_none() {
                        // The Python future was cancelled, but the Rust Task completed anyway.
                        debug!(
                            "CompioFuture completed after being cancelled, result: {:?}",
                            result
                        );
                    } else {
                        // The future was set by the user by mistake, log the exception.
                        let _ = (|| {
                            let context = PyDict::new(py);
                            context.set_item("message", "CompioFuture is already done")?;
                            context.set_item("future", py_fut)?;
                            context.set_item("exception", e)?;
                            if let Ok(result) = result {
                                context.set_item("result", result)?;
                            }
                            runtime::call_exception_handler(py, context)
                        })();
                    }
                }
            })
        });
        cancellable.borrow_mut().task.replace(join_handle);
        Ok(rv)
    }

    pub fn get_ktls_mode(&self) -> KtlsMode {
        self.ktls_mode
    }

    fn socket_to_fd(&self, py: Python, sock: &Py<PyAny>) -> PyResult<SharedFd<BorrowedFd>> {
        let registered = self.registered.bind(py);
        let sock = sock.bind(py);
        let socket_ref = if registered.contains(sock)? {
            registered.get_item(sock)?.cast_into()?
        } else {
            let socket_ref = SocketRef::from_raw(py, sock.call_method0("fileno")?.extract()?)?;
            registered.set_item(sock, &socket_ref)?;
            socket_ref
        };
        Ok(socket_ref.borrow().attacher.to_shared_fd())
    }
}

#[pyclass(unsendable)]
struct Cancellable {
    task: Option<JoinHandle<()>>,
}

#[pymethods]
impl Cancellable {
    fn cancel(&mut self) {
        drop(self.task.take());
    }
}

struct StoreFalseGuard(Arc<AtomicBool>);

impl Drop for StoreFalseGuard {
    #[inline]
    fn drop(&mut self) {
        self.0.store(false, atomic::Ordering::SeqCst);
    }
}

#[cfg(unix)]
type BorrowedFd = std::os::fd::BorrowedFd<'static>;
#[cfg(windows)]
type BorrowedFd = std::os::windows::io::BorrowedHandle<'static>;

#[pyclass(unsendable)]
struct SocketRef {
    attacher: runtime::Attacher<BorrowedFd>,
}

impl SocketRef {
    fn from_raw(py: Python, fd: i32) -> PyResult<Bound<Self>> {
        let fd = unsafe { BorrowedFd::borrow_raw(fd as _) };
        let attacher = runtime::Attacher::new(fd)?;
        Bound::new(py, Self { attacher })
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CompioLoop>()?;

    // CompioFuture is a subclass of asyncio.Future that cancels the associated
    // Rust Future when cancelled from Python. This is only constructed internally,
    // so we don't need to expose it here.
    let def = c_str!(
        r#"
import asyncio

class CompioFuture(asyncio.Future):
    def __init__(self, cancellable):
        super().__init__()
        self._cancellable = cancellable

    def cancel(self, *args, **kwargs):
        self._cancellable.cancel()
        super().cancel(*args, **kwargs)
"#
    );
    let py = m.py();
    let locals = PyDict::new(py);
    py.run(def, None, Some(&locals))?;
    let future_type = locals
        .get_item("CompioFuture")?
        .expect("defined CompioFuture")
        .unbind();
    COMPIO_FUTURE
        .set(future_type)
        .expect("FUTURE_TYPE is empty");

    Ok(())
}
