// SPDX-License-Identifier: Apache-2.0 OR MulanPSL-2.0
// Copyright 2025 Fantix King

use std::{
    cell::RefCell,
    cmp,
    collections::{BinaryHeap, VecDeque},
    io,
    ops::Deref,
    panic,
    pin::Pin,
    sync::{
        Arc,
        atomic::{self, AtomicBool, AtomicUsize},
    },
    task::Waker,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use async_task::{Runnable, Task};
use compio::{
    BufResult,
    buf::IntoInner,
    driver::{
        AsFd, AsRawFd, DriverType, Key, OpCode, Proactor, PushEntry, SharedFd, ToSharedFd,
        op::Asyncify,
    },
};
use compio_log::*;
use pyo3::{exceptions::PyRuntimeError, prelude::*, types::PyDict, types::PyWeakrefReference};

/// Minimum number of _scheduled timer handles before cleanup of
/// cancelled handles is performed.
const MIN_SCHEDULED_TIMER_HANDLES: usize = 100;

/// Minimum fraction of _scheduled timer handles that are cancelled
/// before cleanup of cancelled handles is performed.
const MIN_CANCELLED_TIMER_HANDLES_FRACTION: f64 = 0.5;

/// Maximum timeout passed to select to avoid OS limitations
const MAXIMUM_SELECT_TIMEOUT: Duration = Duration::from_hours(24);

scoped_tls::scoped_thread_local!(static CURRENT_RUNTIME: Runtime);

enum OpOrKey<T> {
    Op(T),
    Key(Key<T>),
}

impl<T> Unpin for OpOrKey<T> {}

struct OpFuture<T> {
    state: Option<OpOrKey<T>>,
}

impl<T: OpCode + 'static> Future for OpFuture<T> {
    type Output = BufResult<usize, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        CURRENT_RUNTIME.with(|runtime| {
            let state = self.state.take().expect("polled after completion");
            let mut driver = runtime.driver.borrow_mut();
            let entry = match state {
                OpOrKey::Op(op) => driver.push(op),
                OpOrKey::Key(key) => driver.pop(key),
            };
            match entry {
                PushEntry::Pending(mut key) => {
                    driver.update_waker(&mut key, cx.waker());
                    drop(driver);
                    self.state.replace(OpOrKey::Key(key));
                    Poll::Pending
                }
                PushEntry::Ready(result) => Poll::Ready(result),
            }
        })
    }
}

#[allow(unused)]
#[derive(Default)]
pub struct Yield(bool);

impl Future for Yield {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.get_mut().0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

struct TimerKey {
    when: Instant,
    waker: Waker,
    cancelled: Arc<AtomicBool>,
}

impl TimerKey {
    #[inline]
    fn cancelled(&self) -> bool {
        self.cancelled.load(atomic::Ordering::Acquire)
    }
}

impl PartialEq for TimerKey {
    fn eq(&self, other: &Self) -> bool {
        self.when == other.when
    }
}

impl Eq for TimerKey {}

impl PartialOrd for TimerKey {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerKey {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        // Reverse order to turn BinaryHeap into a min-heap (smallest `when` first)
        other
            .when
            .partial_cmp(&self.when)
            .unwrap_or(cmp::Ordering::Equal)
    }
}

pub struct Timer {
    when: f64,
    scheduled: bool,
    cancelled: Arc<AtomicBool>,
    timer_cancelled_count: Arc<AtomicUsize>,
}

impl Timer {
    fn new(when: f64, timer_cancelled_count: Arc<AtomicUsize>) -> Self {
        Timer {
            when,
            scheduled: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            timer_cancelled_count,
        }
    }

    #[inline]
    pub fn when(&self) -> f64 {
        self.when
    }
}

impl Future for Timer {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.scheduled {
            Poll::Ready(())
        } else {
            CURRENT_RUNTIME.with(|runtime| {
                let when =
                    runtime.epoch + Duration::try_from_secs_f64(self.when).unwrap_or_default();
                if when < Runtime::end_time() {
                    Poll::Ready(())
                } else {
                    runtime.scheduled.borrow_mut().push(TimerKey {
                        when,
                        waker: cx.waker().clone(),
                        cancelled: self.cancelled.clone(),
                    });
                    trace!("Timer scheduled for {:?}", when);
                    self.get_mut().scheduled = true;
                    Poll::Pending
                }
            })
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        trace!("Timer cancelled");
        if self
            .cancelled
            .compare_exchange(
                false,
                true,
                atomic::Ordering::AcqRel,
                atomic::Ordering::Acquire,
            )
            .is_ok()
            && self.scheduled
        {
            debug!("Timer cancelled counted");
            self.timer_cancelled_count
                .fetch_add(1, atomic::Ordering::Release);
        }
    }
}

pub struct Runtime {
    pyloop: Py<PyWeakrefReference>,
    epoch: Instant,
    stopping: Arc<AtomicBool>,
    driver: RefCell<Proactor>,
    ready: RefCell<VecDeque<Runnable>>,
    scheduled: RefCell<BinaryHeap<TimerKey>>,
    fatal_error: RefCell<Option<PyErr>>,
    timer_cancelled_count: Arc<AtomicUsize>,
}

impl Runtime {
    pub fn new(pyloop: Py<PyWeakrefReference>, stopping: Arc<AtomicBool>) -> io::Result<Runtime> {
        debug!("Creating new Runtime");
        let driver = Proactor::new()?;
        debug!("Runtime created with driver: {:?}", driver.driver_type());
        Ok(Runtime {
            pyloop,
            epoch: Instant::now(),
            stopping,
            driver: RefCell::new(driver),
            ready: RefCell::new(VecDeque::new()),
            scheduled: RefCell::new(BinaryHeap::new()),
            fatal_error: RefCell::new(None),
            timer_cancelled_count: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn driver_type(&self) -> DriverType {
        self.driver.borrow().driver_type()
    }

    pub fn spawn<F>(&self, fut: F) -> Task<F::Output>
    where
        F: Future + 'static,
    {
        let schedule = |runnable| {
            self.ready.borrow_mut().push_back(runnable);
        };
        let (runnable, task) = unsafe { async_task::spawn_unchecked(fut, schedule) };
        runnable.schedule();
        task
    }

    #[inline]
    pub fn since_epoch(&self) -> Duration {
        Instant::now().duration_since(self.epoch)
    }

    pub fn timer(&self, when: f64) -> Timer {
        Timer::new(when, self.timer_cancelled_count.clone())
    }

    #[inline(always)]
    fn run_once(&self) -> PyResult<()> {
        let sched_count = self.scheduled.borrow().len();
        if sched_count > MIN_SCHEDULED_TIMER_HANDLES
            && self.timer_cancelled_count.load(atomic::Ordering::Acquire) as f64
                / sched_count as f64
                > MIN_CANCELLED_TIMER_HANDLES_FRACTION
        {
            // Remove delayed calls that were cancelled if their number
            // is too high
            let mut new_scheduled = Vec::new();
            for key in self.scheduled.take() {
                if !key.cancelled() {
                    new_scheduled.push(key);
                }
            }
            self.scheduled.replace(BinaryHeap::from(new_scheduled));
            self.timer_cancelled_count
                .store(0, atomic::Ordering::Release);
        } else {
            // Remove delayed calls that were cancelled from head of queue.
            let mut scheduled = self.scheduled.borrow_mut();
            while let Some(next_scheduled) = scheduled.peek() {
                if next_scheduled.cancelled() {
                    self.timer_cancelled_count
                        .fetch_sub(1, atomic::Ordering::Release);
                    scheduled.pop();
                } else {
                    break;
                }
            }
        }

        let timeout =
            if !self.ready.borrow().is_empty() || self.stopping.load(atomic::Ordering::Acquire) {
                Some(Duration::default())
            } else if let Some(next_scheduled) = self.scheduled.borrow().peek() {
                Some(
                    next_scheduled
                        .when
                        .saturating_duration_since(Instant::now())
                        .min(MAXIMUM_SELECT_TIMEOUT),
                )
            } else {
                None
            };
        debug!("Polling I/O with timeout {:?}", timeout);
        self.driver
            .borrow_mut()
            .poll(timeout)
            .or_else(|e| match e.kind() {
                io::ErrorKind::TimedOut => Ok(()),
                _ => Err(e),
            })?;

        // Handle 'later' callbacks that are ready.
        {
            let end_time = Self::end_time();
            let mut scheduled = self.scheduled.borrow_mut();
            loop {
                match scheduled.peek() {
                    Some(TimerKey { when, .. }) if when < &end_time => {
                        let TimerKey { waker, .. } = scheduled.pop().expect("not empty");
                        waker.wake();
                    }
                    _ => break,
                }
            }
        }

        // This is the only place where callbacks are actually *called*.
        // All other places just add them to ready.
        // Note: We run all currently scheduled callbacks, but not any
        // callbacks scheduled by callbacks run this time around --
        // they will be run the next time (after another I/O poll).
        // Use an idiom that is thread-safe without using locks.
        let ntodo = self.ready.borrow().len();
        debug!("Ready handles to run: {}", ntodo);
        for _ in 0..ntodo {
            let runnable = self.ready.borrow_mut().pop_front().expect("not empty");
            runnable.run();
            if let Some(err) = self.fatal_error.take() {
                return Err(err);
            }
        }

        Ok(())
    }

    #[inline]
    fn end_time() -> Instant {
        Instant::now()
            + if cfg!(any(target_os = "linux", target_os = "macos")) {
                Duration::from_nanos(1)
            } else if cfg!(target_os = "windows") {
                Duration::from_nanos(100)
            } else {
                Duration::from_nanos(1000)
            }
    }

    pub fn run(&self) -> PyResult<()> {
        CURRENT_RUNTIME.set(self, || {
            loop {
                trace!("Before run_once");
                self.run_once()?;
                trace!("After run_once");
                if self.stopping.load(atomic::Ordering::SeqCst) {
                    debug!("Runtime loop stopped");
                    break Ok(());
                }
            }
        })
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        debug!("Dropping runtime");
        for key in self.scheduled.take() {
            key.waker.wake();
            trace!("Drop TimerKey");
        }
        self.ready.take();
    }
}

pub async fn execute<T>(op: T) -> BufResult<usize, T>
where
    T: OpCode + 'static,
{
    OpFuture {
        state: Some(OpOrKey::Op(op)),
    }
    .await
}

pub async fn asyncify<T, F>(f: F) -> T
where
    T: Send + 'static,
    F: (FnOnce() -> T) + Send + 'static,
{
    let op = Asyncify::new(|| {
        let rv = panic::catch_unwind(panic::AssertUnwindSafe(f));
        BufResult(Ok(0), rv)
    });
    let res = execute(op).await.1.into_inner();
    res.unwrap_or_else(|e| panic::resume_unwind(e))
}

pub fn call_exception_handler(py: Python, context: Bound<PyDict>) -> PyResult<()> {
    CURRENT_RUNTIME.with(|runtime| {
        runtime
            .pyloop
            .bind(py)
            .upgrade()
            .ok_or_else(|| PyRuntimeError::new_err("Event loop is closed"))?
            .getattr("call_exception_handler")?
            .call1((context,))
            .map(drop)
    })
}

pub fn fatal_error(err: PyErr) {
    CURRENT_RUNTIME.with(|runtime| {
        let mut fatal_error = runtime.fatal_error.borrow_mut();
        if fatal_error.is_none() {
            *fatal_error = Some(err);
        }
    });
}

// Attacher is copied and stripped from compio-runtime:
// https://github.com/compio-rs/compio/blob/88846e5e81e5833d53c72b605a90a432a7445de9/compio-runtime/src/attacher.rs
// Copyright (c) 2023 Berrysoft

#[derive(Debug)]
pub struct Attacher<S> {
    source: SharedFd<S>,
}

impl<S> Attacher<S> {
    pub unsafe fn new_unchecked(source: S) -> Self {
        Self {
            source: unsafe { SharedFd::new_unchecked(source) },
        }
    }
}

impl<S: AsFd> Attacher<S> {
    pub fn new(source: S) -> io::Result<Self> {
        CURRENT_RUNTIME.with(|runtime| {
            runtime
                .driver
                .borrow_mut()
                .attach(source.as_fd().as_raw_fd())
        })?;
        Ok(unsafe { Self::new_unchecked(source) })
    }
}

impl<S> IntoInner for Attacher<S> {
    type Inner = SharedFd<S>;

    fn into_inner(self) -> Self::Inner {
        self.source
    }
}

impl<S> Clone for Attacher<S> {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
        }
    }
}

#[cfg(windows)]
impl<S: std::os::windows::io::FromRawHandle> std::os::windows::io::FromRawHandle for Attacher<S> {
    unsafe fn from_raw_handle(handle: std::os::windows::io::RawHandle) -> Self {
        unsafe { Self::new_unchecked(S::from_raw_handle(handle)) }
    }
}

#[cfg(windows)]
impl<S: std::os::windows::io::FromRawSocket> std::os::windows::io::FromRawSocket for Attacher<S> {
    unsafe fn from_raw_socket(sock: std::os::windows::io::RawSocket) -> Self {
        unsafe { Self::new_unchecked(S::from_raw_socket(sock)) }
    }
}

#[cfg(unix)]
impl<S: std::os::fd::FromRawFd> std::os::fd::FromRawFd for Attacher<S> {
    unsafe fn from_raw_fd(fd: std::os::fd::RawFd) -> Self {
        unsafe { Self::new_unchecked(S::from_raw_fd(fd)) }
    }
}

impl<S> Deref for Attacher<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        self.source.deref()
    }
}

impl<S> ToSharedFd<S> for Attacher<S> {
    fn to_shared_fd(&self) -> SharedFd<S> {
        self.source.to_shared_fd()
    }
}
