# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


import time as py_time
import asyncio
import contextvars
import functools
import inspect
import os
import reprlib
import threading
import traceback

cdef asyncio_isfuture = asyncio.isfuture
cdef asyncio_ensure_future = asyncio.ensure_future
cdef asyncio_set_running_loop = asyncio._set_running_loop
cdef asyncio_get_running_loop = asyncio._get_running_loop
cdef asyncio_Task = asyncio.Task
cdef asyncio_Future = asyncio.Future
cdef logger = asyncio.log.logger

cdef long long SECOND_NS = 1_000_000_000
cdef long long MAX_SELECT_TIMEOUT = 24 * 3600 * SECOND_NS

# Minimum number of scheduled timer handles before cleanup of
# cancelled handles is performed.
cdef int MIN_SCHEDULED_TIMER_HANDLES = 100

# Maximum ratio of cancelled handles is performed of scheduled timer handles
# that are cancelled before cleanup
cdef int MAX_CANCELLED_TIMER_HANDLES_RATIO = 2

include "handle.pyx"
include "queue.pyx"
include "heapq.pyx"
include "uring.pyx"
include "tcp.pyx"
include "udp.pyx"
include "fileio.pyx"
include "resolver.pyx"


cdef long long monotonic_ns() nogil except -1:
    cdef:
        long long rv
        time.timespec ts
    if time.clock_gettime(time.CLOCK_MONOTONIC, &ts):
        with gil:
            PyErr_SetFromErrno(OSError)
        return -1
    rv = ts.tv_sec * SECOND_NS
    return rv + ts.tv_nsec


cdef int loop_init(
    Loop* loop, linux.__u32 depth, linux.io_uring_params* params
) nogil except 0:
    if not queue_init(&loop.ready):
        return 0
    if not heapq_init(&loop.scheduled):
        queue_uninit(&loop.ready)
        return 0
    if not ring_init(&loop.ring, depth, params):
        queue_uninit(&loop.ready)
        heapq_uninit(&loop.scheduled)
        return 0
    return 1


cdef int loop_uninit(Loop* loop) nogil except 0:
    heapq_uninit(&loop.scheduled)
    queue_uninit(&loop.ready)
    return ring_uninit(&loop.ring)


cdef int loop_run_forever(Loop* loop) nogil except 0:
    cdef:
        Ring* ring = &loop.ring
        Queue* ready = &loop.ready
        HeapQueue* scheduled = &loop.scheduled

    while True:
        if not loop_run_once(loop, ring, ready, scheduled):
            return 0
        if loop.stopping:
            break
    return 1


cdef inline int filter_cancelled_calls(Loop* loop) nogil except 0:
    cdef:
        HeapQueue* scheduled = &loop.scheduled
        HeapQueue heap, drop
        Callback** array = scheduled.array
        Callback* callback
        int i = 0, size = scheduled.tail

    if (
        MIN_SCHEDULED_TIMER_HANDLES < size <
        loop.timer_cancelled_count * MAX_CANCELLED_TIMER_HANDLES_RATIO
    ):
        # Remove delayed calls that were cancelled if their number
        # is too high
        if not heapq_init(&drop):
            return 0
        if not heapq_init(&heap):
            heapq_uninit(&drop)
            return 0
        while i < size:
            callback = array[i]
            if callback.mask & CANCELLED_MASK:
                callback.mask &= ~SCHEDULED_MASK
                if not heapq_push(&drop, callback, 0):
                    heap.tail = 0
                    heapq_uninit(&heap)
                    drop.tail = 0
                    heapq_uninit(&drop)
                    return 0
            elif not heapq_push(&heap, callback, 0):
                heap.tail = 0
                heapq_uninit(&heap)
                drop.tail = 0
                heapq_uninit(&drop)
                return 0
        heapq_heapify(&heap)
        heap, scheduled[0] = scheduled[0], heap
        heap.tail = 0
        heapq_uninit(&heap)
        heapq_uninit(&drop)
    elif array[0].mask & CANCELLED_MASK:
        if not heapq_init(&drop):
            return 0
        while size:
            callback = heapq_pop(scheduled)
            if callback.mask & CANCELLED_MASK:
                loop.timer_cancelled_count -= 1
                callback.mask &= ~SCHEDULED_MASK
                if not heapq_push(&drop, callback, 0):
                    with gil:
                        Py_DECREF(<object>callback.handle)
                        heapq_uninit(&drop)
                    return 0
            if not array[0].mask & CANCELLED_MASK:
                break
            size -= 1
        heapq_uninit(&drop)

    return 1


cdef loop_run_ready(Queue* ready, int ntodo):
    cdef Handle handle

    while ntodo:
        handle = queue_pop_py(ready)
        if not handle.cb.mask & CANCELLED_MASK:
            handle.run()
        ntodo -= 1
    handle = None


cdef inline int loop_run_once(
    Loop* loop, Ring* ring, Queue* ready, HeapQueue* scheduled
) nogil except 0:
    cdef:
        Callback* callback
        long long timeout = -1, now
        int nready
        RingCallback* cb = NULL

    if scheduled.tail:
        if not filter_cancelled_calls(loop):
            return 0

    if ready.head >= 0 or loop.stopping:
        timeout = 0
    elif scheduled.tail:
        timeout = min(
            max(0, scheduled.array[0].when - monotonic_ns()),
            MAX_SELECT_TIMEOUT,
        )

    nready = ring_select(ring, timeout)
    if nready < 0:
        return 0
    while nready:
        ring_cq_pop(&ring.cq, &cb)
        if cb != NULL and not cb.callback(cb):
            return 0
        nready -= 1

    now = monotonic_ns() + 1
    while scheduled.tail and scheduled.array[0].when < now:
        callback = heapq_pop(scheduled)
        callback.mask &= ~SCHEDULED_MASK
        if not queue_push(ready, callback):
            if not heapq_push(scheduled, callback, 1):
                with gil:
                    Py_DECREF(<object>callback.handle)
            return 0

    if ready.head >= 0:
        with gil:
            loop_run_ready(ready, queue_size(ready))
    return 1


class KLoopPolicy(asyncio.events.BaseDefaultEventLoopPolicy):
    __slots__ = ("_selector_args",)

    def __init__(
        self, queue_depth=128, sq_thread_idle=2000, sq_thread_cpu=None
    ):
        super().__init__()
        self._selector_args = (queue_depth, sq_thread_idle, sq_thread_cpu)

    def _loop_factory(self):
        return KLoop(*self._selector_args)

    # Child processes handling (Unix only).

    def get_child_watcher(self):
        raise NotImplementedError

    def set_child_watcher(self, watcher):
        raise NotImplementedError


cdef class KLoopImpl:
    def __init__(self, queue_depth, sq_thread_idle, sq_thread_cpu):
        cdef:
            linux.io_uring_params params
            linux.__u32 depth
        string.memset(&params, 0, sizeof(params))
        params.flags = linux.IORING_SETUP_SQPOLL
        params.sq_thread_idle = sq_thread_idle
        if sq_thread_cpu is not None:
            params.sq_thread_cpu = sq_thread_cpu
            params.flags |= linux.IORING_SETUP_SQ_AFF
        depth = queue_depth
        self.loop.loop = <PyObject*>self
        with nogil:
            loop_init(&self.loop, depth, &params)

        self.resolver = Resolver.new(self)
        self.closed = False
        self.thread_id = None

    def __dealloc__(self):
        with nogil:
            loop_uninit(&self.loop)

    cdef inline check_closed(self):
        if self.closed:
            raise RuntimeError('Event loop is closed')

    cdef inline bint _is_running(self):
        return self.thread_id is not None

    cdef inline check_running(self):
        if self._is_running():
            raise RuntimeError('This event loop is already running')
        if asyncio_get_running_loop() is not None:
            raise RuntimeError(
                'Cannot run the event loop while another loop is running')

    def run_forever(self):
        """Run until stop() is called."""
        self.check_closed()
        self.check_running()
        # self._set_coroutine_origin_tracking(self._debug)
        self.thread_id = threading.get_ident()

        # old_agen_hooks = sys.get_asyncgen_hooks()
        # sys.set_asyncgen_hooks(firstiter=self._asyncgen_firstiter_hook,
        #                        finalizer=self._asyncgen_finalizer_hook)
        try:
            asyncio_set_running_loop(self)
            with nogil:
                loop_run_forever(&self.loop)
        finally:
            self.loop.stopping = 0
            self.thread_id = None
            asyncio_set_running_loop(None)
            # self._set_coroutine_origin_tracking(False)
            # sys.set_asyncgen_hooks(*old_agen_hooks)

    def run_until_complete(self, future):
        self.check_closed()
        self.check_running()

        new_task = not asyncio_isfuture(future)
        future = asyncio_ensure_future(future, loop=self)
        if new_task:
            # An exception is raised if the future didn't complete, so there
            # is no need to log the "destroy pending task" message
            future._log_destroy_pending = False

        future.add_done_callback(_run_until_complete_cb)
        try:
            self.run_forever()
        except:
            if new_task and future.done() and not future.cancelled():
                # The coroutine raised a BaseException. Consume the exception
                # to not log a warning, the caller doesn't have access to the
                # local task.
                future.exception()
            raise
        finally:
            future.remove_done_callback(_run_until_complete_cb)
        if not future.done():
            raise RuntimeError('Event loop stopped before Future completed.')

        return future.result()

    def create_task(self, coro, *, name=None):
        self.check_closed()
        # if self._task_factory is None:
        task = asyncio_Task(coro, loop=self, name=name)
        if task._source_traceback:
            del task._source_traceback[-1]
        # else:
        #     task = self._task_factory(self, coro)
        #     tasks._set_task_name(task, name)

        return task

    def stop(self):
        self.loop.stopping = 1

    def close(self):
        if self.is_running():
            raise RuntimeError("Cannot close a running event loop")
        if self.closed:
            return
        # if self._debug:
        #     logger.debug("Close %r", self)
        self.closed = True
        # self.ready.clear()
        # self._scheduled.clear()
        # self._executor_shutdown_called = True
        # executor = self._default_executor
        # if executor is not None:
        #     self._default_executor = None
        #     executor.shutdown(wait=False)

    def fileno(self):
        return self.loop.ring.ring_fd

    def is_running(self):
        return self._is_running()

    def get_debug(self):
        return False

    def call_soon(self, callback, *args, context=None):
        cdef Handle handle
        self.check_closed()
        # if self._debug:
        #     self._check_thread()
        #     self._check_callback(callback, 'call_soon')
        handle = self._call_soon(callback, args, context)
        if handle.source_traceback:
            del handle.source_traceback[-1]
        return handle

    def time(self):
        return (<float>monotonic_ns()) / SECOND_NS

    def call_later(self, delay, callback, *args, context=None):
        cdef long long when = monotonic_ns()
        when += delay * SECOND_NS
        timer = self._call_at(when, callback, args, context)
        if timer.source_traceback:
            del timer.source_traceback[-1]
        return timer

    def call_at(self, when, callback, *args, context=None):
        timer = self._call_at(when * SECOND_NS, callback, args, context)
        if timer.source_traceback:
            del timer.source_traceback[-1]
        return timer

    cdef inline TimerHandle _call_at(
        self, long long when, callback, args, context
    ):
        cdef TimerHandle timer
        self.check_closed()
        # if self._debug:
        #     self._check_thread()
        #     self._check_callback(callback, 'call_at')
        timer = TimerHandle(when, callback, args, self, context)
        heapq_push_py(&self.loop.scheduled, timer)
        # else:
        #     heapq_heappush(self.heapq)
        timer.cb.mask |= SCHEDULED_MASK
        return timer

    cdef inline Handle _call_soon(self, callback, args, context):
        cdef Handle handle = Handle(callback, args, self, context)
        self._add_callback(handle)
        return handle

    cdef inline _add_callback(self, Handle handle):
        queue_push_py(&self.loop.ready, handle)

    def default_exception_handler(self, context):
        message = context.get('message')
        if not message:
            message = 'Unhandled exception in event loop'

        exception = context.get('exception')
        if exception is not None:
            exc_info = (type(exception), exception, exception.__traceback__)
        else:
            exc_info = False

        # if ('source_traceback' not in context and
        #         self._current_handle is not None and
        #         self._current_handle._source_traceback):
        #     context['handle_traceback'] = \
        #         self._current_handle._source_traceback

        log_lines = [message]
        for key in sorted(context):
            if key in {'message', 'exception'}:
                continue
            value = context[key]
            if key == 'source_traceback':
                tb = ''.join(traceback.format_list(value))
                value = 'Object created at (most recent call last):\n'
                value += tb.rstrip()
            elif key == 'handle_traceback':
                tb = ''.join(traceback.format_list(value))
                value = 'Handle created at (most recent call last):\n'
                value += tb.rstrip()
            else:
                value = repr(value)
            log_lines.append(f'{key}: {value}')

        logger.error('\n'.join(log_lines), exc_info=exc_info)

    def call_exception_handler(self, context):
        # if self._exception_handler is None:
        try:
            self.default_exception_handler(context)
        except (SystemExit, KeyboardInterrupt):
            raise
        except BaseException:
            # Second protection layer for unexpected errors
            # in the default implementation, as well as for subclassed
            # event loops with overloaded "default_exception_handler".
            logger.error('Exception in default exception handler',
                         exc_info=True)
        # else:
        #     try:
        #         self._exception_handler(self, context)
        #     except (SystemExit, KeyboardInterrupt):
        #         raise
        #     except BaseException as exc:
        #         # Exception in the user set custom exception handler.
        #         try:
        #             # Let's try default handler.
        #             self.default_exception_handler({
        #                 'message': 'Unhandled error in exception handler',
        #                 'exception': exc,
        #                 'context': context,
        #             })
        #         except (SystemExit, KeyboardInterrupt):
        #             raise
        #         except BaseException:
        #             # Guard 'default_exception_handler' in case it is
        #             # overloaded.
        #             logger.error('Exception in default exception handler '
        #                          'while handling an unexpected error '
        #                          'in custom exception handler',
        #                          exc_info=True)

    async def shutdown_asyncgens(self):
        pass

    async def shutdown_default_executor(self):
        pass

    cpdef create_future(self):
        return asyncio_Future(loop=self)

    def _timer_handle_cancelled(self, handle):
        pass

    async def create_connection(
        self,
        protocol_factory,
        host=None,
        port=None,
        *,
        ssl=None,
        family=0,
        proto=0,
        flags=0,
        sock=None,
        local_addr=None,
        server_hostname=None,
        ssl_handshake_timeout=None,
        happy_eyeballs_delay=None,
        interleave=None,
    ):
        cdef:
            int fd

        if ssl is False:
            ssl = None
        elif ssl is not None:
            from . import tls
            if ssl is True:
                import ssl as ssl_mod
                ssl = ssl_mod.create_default_context()
        fd = await tcp_connect(self, host, port)
        protocol = protocol_factory()
        if ssl is not None:
            transport = tls.TLSTransport.new(fd, protocol, self, ssl)
        else:
            transport = TCPTransport.new(fd, protocol, self)
        return transport, protocol


class KLoop(KLoopImpl, asyncio.AbstractEventLoop):
    pass


def _run_until_complete_cb(fut):
    if not fut.cancelled():
        exc = fut.exception()
        if isinstance(exc, (SystemExit, KeyboardInterrupt)):
            # Issue #22429: run_forever() already finished, no need to
            # stop it.
            return
    _get_loop(fut).stop()


def _get_loop(fut):
    # Tries to call Future.get_loop() if it's available.
    # Otherwise fallbacks to using the old '_loop' property.
    try:
        get_loop = fut.get_loop
    except AttributeError:
        pass
    else:
        return get_loop()
    return fut._loop
