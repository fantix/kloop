use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    net::{Ipv4Addr, SocketAddr},
    ops::Deref,
    slice,
    sync::atomic::AtomicU8,
};

use compio::buf::{BufResult, IoVectoredBuf, buf_try};
use compio_executor::JoinHandle;
use pyo3::{
    BoundObject, IntoPyObject, IntoPyObjectExt,
    buffer::PyBuffer,
    exceptions::{PyOSError, PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyString, PyTuple},
};
use socket2::{Domain, Type};

use super::Socket;
use crate::{
    event_loop::CompioLoop,
    runtime::{self, RecvStream},
};

struct BufferedProtocol {
    get_buffer: Py<PyAny>,
    buffer_updated: Py<PyAny>,
    buffer: Option<Py<PyAny>>,
}

impl BufferedProtocol {
    fn extract(protocol: &Bound<PyAny>) -> PyResult<Option<Self>> {
        Ok(
            match (
                protocol.getattr_opt("get_buffer")?,
                protocol.getattr_opt("buffer_updated")?,
            ) {
                (Some(get_buffer), Some(buffer_updated)) => Some(Self {
                    get_buffer: get_buffer.unbind(),
                    buffer_updated: buffer_updated.unbind(),
                    buffer: None,
                }),
                _ => None,
            },
        )
    }

    fn clone_ref(&self, py: Python) -> Self {
        debug_assert!(self.buffer.is_none());
        Self {
            get_buffer: self.get_buffer.clone_ref(py),
            buffer_updated: self.buffer_updated.clone_ref(py),
            buffer: None,
        }
    }
}

enum DataCallback {
    Copied(Py<PyAny>),
    Buffered(BufferedProtocol),
}

impl DataCallback {
    fn extract(protocol: &Bound<PyAny>) -> PyResult<Self> {
        Ok(match BufferedProtocol::extract(protocol)? {
            Some(proto) => Self::Buffered(proto),
            None => Self::Copied(protocol.getattr("data_received")?.unbind()),
        })
    }
}

struct StreamProtocol {
    data_callback: DataCallback,
    pause_writing: Py<PyAny>,
    resume_writing: Py<PyAny>,
    protocol: Py<PyAny>,
}

impl StreamProtocol {
    fn new(protocol: Bound<PyAny>) -> PyResult<Self> {
        Ok(Self {
            data_callback: DataCallback::extract(&protocol)?,
            pause_writing: protocol.getattr("pause_writing")?.unbind(),
            resume_writing: protocol.getattr("resume_writing")?.unbind(),
            protocol: protocol.unbind(),
        })
    }
}

enum PyBuf {
    Contiguous(#[allow(dead_code)] PyBuffer<u8>, &'static [u8]),
    Original(PyBuffer<u8>),
    Copied(Vec<u8>),
}

impl PyBuf {
    fn len(&self) -> usize {
        match self {
            Self::Contiguous(_, slice) => slice.len(),
            Self::Original(pybuf) => pybuf.len_bytes(),
            Self::Copied(buf) => buf.len(),
        }
    }
}

#[derive(Default)]
struct VecPyBuf {
    offset: usize,
    buffers: VecDeque<UnsafeCell<PyBuf>>,
    total_size: usize,
}

impl VecPyBuf {
    fn push(&mut self, buf: PyBuffer<u8>) -> PyResult<()> {
        self.total_size = self
            .total_size
            .checked_add(buf.len_bytes())
            .ok_or_else(|| PyRuntimeError::new_err("buffer overflow"))?;
        let buf = if buf.is_c_contiguous() {
            let view = unsafe { slice::from_raw_parts(buf.buf_ptr().cast(), buf.len_bytes()) };
            PyBuf::Contiguous(buf, view)
        } else {
            PyBuf::Original(buf)
        };
        self.buffers.push_back(UnsafeCell::new(buf));
        Ok(())
    }

    fn consume(&mut self, mut len: usize) {
        if len == self.total_size {
            self.buffers.clear();
            self.offset = 0;
            self.total_size = 0;
        } else {
            while let Some(buf) = self.buffers.front_mut() {
                let buf_len = buf.get_mut().len();
                match len.checked_sub(buf_len) {
                    Some(remainder) => {
                        len = remainder;
                        self.buffers.pop_front();
                        self.total_size -= buf_len;
                    }
                    None => break,
                }
            }
            self.offset = len;
        }
    }

    fn extend(&mut self, other: Self) {
        self.buffers.extend(other.buffers);
        self.total_size += other.total_size;
    }
}

impl IoVectoredBuf for VecPyBuf {
    fn iter_slice(&self) -> impl Iterator<Item = &[u8]> {
        VecPyBufIterator {
            deque: &self.buffers,
            offset: 0,
        }
    }

    fn total_len(&self) -> usize {
        self.total_size
    }
}

struct VecPyBufIterator<'a> {
    deque: &'a VecDeque<UnsafeCell<PyBuf>>,
    offset: usize,
}

impl VecPyBufIterator<'_> {
    fn len(&self) -> usize {
        self.deque.len() - self.offset
    }
}

impl<'a> Iterator for VecPyBufIterator<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.nth(0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }

    fn count(self) -> usize {
        self.len()
    }

    fn last(mut self) -> Option<Self::Item> {
        self.nth(self.len() - 1)
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let item = self.deque.get(self.offset + n);
        self.offset = self.deque.len().min(self.offset + n + 1);
        let pybuf = unsafe { &mut *item?.get() };
        if let PyBuf::Original(buf) = &*pybuf {
            *pybuf =
                PyBuf::Copied(Python::attach(|py| buf.to_vec(py)).expect("should be compatible"));
        }
        Some(match pybuf {
            PyBuf::Contiguous(_, view) => view,
            PyBuf::Copied(buf) => buf.as_slice(),
            PyBuf::Original(_) => unreachable!(),
        })
    }
}

#[repr(u8)]
enum TransportState {
    Active,
    Closing,
    Closed,
}

#[pyclass(unsendable)]
pub struct StreamTransport {
    pyloop: Py<CompioLoop>,
    inner: Option<Socket>,
    protocol: StreamProtocol,
    outgoing: VecPyBuf,
    state: AtomicU8,
    receiver: Option<JoinHandle<()>>,
    sender: Option<JoinHandle<()>>,
}

impl StreamTransport {
    pub async fn new(
        pyloop: Py<CompioLoop>,
        host: String,
        port: u16,
        protocol_factory: Py<PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let ip = host.parse::<Ipv4Addr>()?;
        let addr = SocketAddr::new(ip.into(), port).into();
        let socket = Socket::new(Domain::IPV4, Type::STREAM, None).await?;

        let (mut transport, protocol) = Python::attach(|py| {
            let protocol = protocol_factory.bind(py).call0()?;
            let transport = StreamTransport {
                pyloop: pyloop.clone_ref(py),
                inner: Some(socket.clone()),
                protocol: StreamProtocol::new(protocol.clone())?,
                outgoing: VecPyBuf::default(),
                state: AtomicU8::new(0),
                receiver: None,
                sender: None,
            };
            Py::new(py, transport).map(|tr| (tr, protocol.unbind()))
        })?;

        socket.connect_async(&addr).await?;

        Python::attach(|py| {
            let pyloop = pyloop.bind(py).borrow();
            let runtime = pyloop.runtime()?;
            let args = (transport.clone_ref(py),);
            Self::schedule_protocol_callback(&mut transport, "connection_made", args);
            let join_handle = match &transport.borrow(py).protocol.data_callback {
                DataCallback::Buffered(proto) => runtime.spawn(Self::receiver_buffered(
                    transport.clone_ref(py),
                    socket,
                    proto.clone_ref(py),
                )),
                DataCallback::Copied(callback) => {
                    let recv_stream = runtime.recv_stream(socket)?;
                    runtime.spawn(Self::receiver_copied(
                        transport.clone_ref(py),
                        recv_stream,
                        callback.clone_ref(py),
                    ))
                }
            };
            transport.borrow_mut(py).receiver = Some(join_handle);
            (transport, protocol).into_py_any(py)
        })
    }

    async fn receiver_copied(
        mut slf: Py<Self>,
        mut recv_stream: RecvStream<Socket>,
        callback: Py<PyAny>,
    ) {
        let mut receiver = async || loop {
            match recv_stream.next().await? {
                Some(buffer) => {
                    Python::attach(|py| callback.call1(py, (buffer.deref(),)))?;
                }
                None => {
                    Self::schedule_connection_lost(&mut slf, None);
                    break Ok(());
                }
            }
        };
        if let Err(e) = receiver().await {
            Self::schedule_connection_lost(&mut slf, Some(e));
        }
    }

    async fn receiver_buffered(mut slf: Py<Self>, socket: Socket, mut proto: BufferedProtocol) {
        let mut receiver = async || loop {
            let buf = Python::attach(|py| {
                let buf = proto.get_buffer.call1(py, (16384,))?;
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
                proto.buffer = Some(buf);
                let ptr = pybuf.buf_ptr() as *mut u8;
                let len = pybuf.len_bytes();
                Ok(unsafe { slice::from_raw_parts_mut(ptr, len) })
            })?;
            let (n, _) = buf_try!(@try socket.recv(buf, 0).await);
            if n == 0 {
                Self::schedule_connection_lost(&mut slf, None);
                break Ok(());
            }
            Python::attach(|py| proto.buffer_updated.call1(py, (n,)).map(drop))?;
        };
        if let Err(e) = receiver().await {
            Self::schedule_connection_lost(&mut slf, Some(e));
        }
    }

    async fn sender(mut slf: Py<Self>, socket: Socket) {
        let mut buf = VecPyBuf::default();
        while Python::attach(|py| {
            let mut this = slf.borrow_mut(py);
            buf.extend(std::mem::take(&mut this.outgoing));
            let done = buf.buffers.is_empty();
            if done && this.inner.is_none() {
                drop(this);
                Self::schedule_connection_lost(&mut slf, None);
            }
            !done
        }) {
            let res;
            BufResult(res, buf) = socket.sendmsg(buf, 0).await;
            match res {
                Ok(0) => {
                    Self::schedule_connection_lost(
                        &mut slf,
                        Some(PyOSError::new_err("broken pipe")),
                    );
                    break;
                }
                Ok(len) => buf.consume(len),
                Err(e) => {
                    Self::schedule_connection_lost(&mut slf, Some(e.into()));
                    break;
                }
            }
        }
    }

    fn schedule_connection_lost(slf: &mut Py<Self>, exc: Option<PyErr>) {
        Python::attach(|py| {
            let mut this = slf.borrow_mut(py);
            this.sender = None;
            this.receiver = None;
            this.inner = None;
            drop(this);
            Self::schedule_protocol_callback(slf, "connection_lost", (exc,));
        });
    }

    fn schedule_protocol_callback<N, ARGS>(slf: &mut Py<Self>, method: N, args: ARGS)
    where
        N: for<'a> IntoPyObject<'a, Target = PyString>,
        ARGS: for<'a> IntoPyObject<'a, Target = PyTuple>,
    {
        if let Err(e) = Python::attach(|py| {
            let slf = slf.borrow(py);
            let handle = crate::handle::Handle::new(
                py,
                slf.protocol.protocol.getattr(py, method)?,
                args.into_pyobject_or_pyerr(py)?.unbind(),
                None,
            )?;
            handle
                .schedule_soon(slf.pyloop.borrow(py).runtime()?.deref(), py)
                .map(drop)
        }) {
            runtime::fatal_error(e);
        }
    }
}

#[pymethods]
impl StreamTransport {
    fn write(slf: &Bound<Self>, py: Python, data: Bound<PyAny>) -> PyResult<()> {
        let mut this = slf.try_borrow_mut()?;
        let slf = slf.clone().unbind();
        this.outgoing.push(PyBuffer::get(&data)?)?;
        if this.sender.is_none() {
            let socket = this
                .inner
                .clone()
                .ok_or_else(|| PyOSError::new_err("transport is closed"))?;
            let join_handle = this
                .pyloop
                .borrow(py)
                .runtime()?
                .spawn(Self::sender(slf, socket));
            this.sender = Some(join_handle);
        }
        Ok(())
    }

    fn close(mut slf: Py<Self>, py: Python) {
        let mut this = slf.borrow_mut(py);
        this.receiver = None;
        this.inner = None;
        if this.sender.is_none() {
            drop(this);
            Self::schedule_connection_lost(&mut slf, None);
        }
    }

    fn abort(&mut self) {
        self.receiver = None;
        self.sender = None;
        self.inner = None;
    }
}
