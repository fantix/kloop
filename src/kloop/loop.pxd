# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


from cpython cimport PyErr_SetFromErrno
from cpython cimport PyMem_RawMalloc, PyMem_RawFree, PyMem_RawRealloc
from cpython cimport PyObject, Py_INCREF, Py_DECREF
from libc cimport errno, string
from posix cimport mman, unistd, time
from posix.types cimport mode_t

from .includes cimport atomic, libc, linux

include "./handle.pxd"
include "./queue.pxd"
include "./heapq.pxd"
include "./uring.pxd"
include "./tcp.pxd"
include "./udp.pxd"
include "./fileio.pxd"
include "./resolver.pxd"


cdef struct Loop:
    bint stopping
    Ring ring
    HeapQueue scheduled
    Queue ready
    int timer_cancelled_count
    PyObject* loop
    CResolver resolver


cdef class KLoopImpl:
    cdef:
        bint closed
        object thread_id
        Loop loop

    cpdef create_future(self)
    cdef inline check_closed(self)
    cdef inline bint _is_running(self)
    cdef inline check_running(self)
    cdef inline Handle _call_soon(self, callback, args, context)
    cdef inline _add_callback(self, Handle handle)
    cdef inline TimerHandle _call_at(
        self, long long when, callback, args, context
    )
