// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2025 Fantix King

use std::iter;

use compio_executor::JoinHandle;
use pyo3::{
    exceptions::{PyKeyboardInterrupt, PySystemExit},
    prelude::*,
    pyclass::CompareOp,
    types::{PyDict, PyTuple},
};

use crate::{import, runtime, runtime::Runtime};

#[pyclass(subclass, weakref, unsendable)]
pub struct Handle {
    callback: Py<PyAny>,
    args: Py<PyTuple>,
    context: Py<PyAny>,
    task: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn new(
        py: Python,
        callback: Py<PyAny>,
        args: Py<PyTuple>,
        context: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        // If no context is provided, copy the current context
        let context = match context {
            Some(ctx) => ctx,
            None => import::contextvars::copy_context(py)?.unbind(),
        };
        Ok(Self {
            callback,
            args,
            context,
            task: None,
        })
    }

    pub fn schedule_soon<'py>(
        self,
        runtime: &Runtime,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, Self>> {
        let rv = Bound::new(py, self)?;
        let slf = rv.clone().unbind();
        rv.borrow_mut()
            .task
            .replace(runtime.spawn(async { Self::run(slf) }));
        Ok(rv)
    }

    pub fn schedule_at<'py>(
        self,
        runtime: &Runtime,
        py: Python<'py>,
        when: f64,
    ) -> PyResult<Bound<'py, TimerHandle>> {
        let timer = runtime.timer(when);
        let slf = TimerHandle(when);
        let initializer = PyClassInitializer::from(self).add_subclass(slf);
        let rv = Bound::new(py, initializer)?;
        let base = rv.as_super();
        let handle = base.clone().unbind();
        base.borrow_mut().task.replace(runtime.spawn(async {
            timer.await;
            Self::run(handle);
        }));
        Ok(rv)
    }

    fn run(slf: Py<Self>) {
        Python::attach(|py| {
            let slf = slf.bind(py);
            if let Err(e) = slf.borrow().run_inner(py, slf) {
                runtime::fatal_error(e);
            }
        })
    }

    #[inline(always)]
    fn run_inner(&self, py: Python, slf: &Bound<Self>) -> PyResult<()> {
        // Build argument list: [callback, arg1, arg2, ...]
        let args = iter::once(self.callback.bind(py).clone())
            .chain(self.args.bind(py).iter())
            .collect::<Vec<_>>();

        // Equivalent to: self.context.run(callback, *args)
        let rv = self
            .context
            .bind(py)
            .getattr("run")
            .and_then(|run| run.call1(PyTuple::new(py, args)?));

        // Handle exceptions
        match rv {
            Ok(_) => Ok(()),

            // Reraise SystemExit and KeyboardInterrupt
            Err(e) if e.is_instance_of::<PySystemExit>(py) => Err(e),
            Err(e) if e.is_instance_of::<PyKeyboardInterrupt>(py) => Err(e),

            // Call the loop's exception handler for other exceptions
            Err(e) => {
                let context = PyDict::new(py);
                context.set_item("message", "Exception in callback")?;
                context.set_item("exception", e)?;
                context.set_item("handle", slf)?;
                runtime::call_exception_handler(py, context)
            }
        }
    }
}

#[pymethods]
impl Handle {
    fn cancel(&mut self) {
        drop(self.task.take())
    }

    fn cancelled(&self) -> bool {
        self.task.is_none()
    }

    fn get_context<'py>(&self, py: Python<'py>) -> Bound<'py, PyAny> {
        self.context.bind(py).clone()
    }
}

#[pyclass(extends=Handle)]
pub struct TimerHandle(f64);

#[pymethods]
impl TimerHandle {
    #[inline]
    fn when(&self) -> f64 {
        self.0
    }

    fn __hash__(&self) -> u64 {
        self.0.to_bits()
    }

    fn __richcmp__(
        slf: &Bound<Self>,
        other: &Bound<Self>,
        op: CompareOp,
        py: Python,
    ) -> PyResult<bool> {
        Ok(match op {
            CompareOp::Eq => Self::eq(slf, other, py)?,
            CompareOp::Ne => !Self::eq(slf, other, py)?,
            CompareOp::Lt => slf.borrow().when() < other.borrow().when(),
            CompareOp::Le => {
                slf.borrow().when() < other.borrow().when() || Self::eq(slf, other, py)?
            }
            CompareOp::Gt => slf.borrow().when() > other.borrow().when(),
            CompareOp::Ge => {
                slf.borrow().when() > other.borrow().when() || Self::eq(slf, other, py)?
            }
        })
    }
}

impl TimerHandle {
    fn eq(slf: &Bound<Self>, other: &Bound<Self>, py: Python) -> PyResult<bool> {
        if slf.borrow().when() == other.borrow().when() {
            let this = slf.as_super().borrow();
            let that = other.as_super().borrow();
            Ok(this.callback.bind(py).eq(that.callback.bind(py))?
                && this.args.bind(py).eq(that.args.bind(py))?
                && this.cancelled() == that.cancelled())
        } else {
            Ok(false)
        }
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Handle>()?;
    m.add_class::<TimerHandle>()?;
    Ok(())
}
