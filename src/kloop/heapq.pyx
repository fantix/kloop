# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef int HEAP_BLOCK_SIZE = 1024


cdef void siftup(HeapQueue* heap, int pos, int endpos) nogil:
    cdef:
        int childpos, limit = endpos >> 1
        Callback** array = heap.array

    while pos < limit:
        childpos = 2 * pos + 1
        if childpos + 1 < endpos:
            if array[childpos].when >= array[childpos + 1].when:
                childpos += 1
        array[childpos], array[pos] = array[pos], array[childpos]
        pos = childpos
    siftdown(heap, pos, endpos)


cdef void siftdown(HeapQueue* heap, int pos, int size) nogil:
    cdef:
        int parentpos
        Callback** array = heap.array
        long long new_when = array[pos].when
    while pos > 0:
        parentpos = (pos - 1) >> 1
        if new_when >= array[parentpos].when:
            break
        new_when = array[pos].when
        array[pos], array[parentpos] = array[parentpos], array[pos]
        pos = parentpos


cdef int heapq_init(HeapQueue* heap) nogil except 0:
    heap.array = <Callback**>PyMem_RawMalloc(
        sizeof(Callback*) * HEAP_BLOCK_SIZE
    )
    if heap.array == NULL:
        with gil:
            raise MemoryError
    heap.size = HEAP_BLOCK_SIZE
    heap.tail = 0
    return 1


cdef void heapq_uninit(HeapQueue* heap) nogil:
    cdef:
        int i = 0, tail = heap.tail
        Callback** array = heap.array

    if array == NULL:
        return
    if i < tail:
        with gil:
            while i < tail:
                Py_DECREF(<object>array[i].handle)
    PyMem_RawFree(array)
    heap.array = NULL


cdef heapq_push_py(HeapQueue* heap, Handle handle):
    cdef Callback* callback = &handle.cb
    Py_INCREF(handle)
    with nogil:
        heapq_push(heap, callback, 1)


cdef int heapq_push(
    HeapQueue* heap, Callback* callback, int keep
) nogil except 0:
    cdef:
        int size = heap.size, tail = heap.tail
        Callback** array = heap.array

    if tail == size:
        size += HEAP_BLOCK_SIZE
        array = <Callback**>PyMem_RawRealloc(array, sizeof(Callback*) * size)
        if array == NULL:
            with gil:
                raise MemoryError
        heap.size = size
    array[tail] = callback
    size = heap.tail = tail + 1
    if keep:
        siftdown(heap, tail, size)
    return 1


cdef Handle heapq_pop_py(HeapQueue* heap):
    cdef:
        Handle handle
        Callback* callback

    with nogil:
        callback = heapq_pop(heap)
    if callback == NULL:
        return None
    else:
        handle = <Handle>callback.handle
        Py_DECREF(handle)
        return handle


cdef Callback* heapq_pop(HeapQueue* heap) nogil:
    cdef:
        Callback* rv
        Callback** array = heap.array
        int size = heap.size, tail = heap.tail

    if tail == 0:
        return NULL

    tail = heap.tail = tail - 1
    rv = array[tail]

    if tail == 0:
        if size > HEAP_BLOCK_SIZE:
            size = HEAP_BLOCK_SIZE
            if PyMem_RawRealloc(array, sizeof(Callback*) * size) != NULL:
                heap.size = size
        return rv

    rv, array[0] = array[0], rv
    if tail > 1:
        siftup(heap, 0, tail)
        if size - tail >= HEAP_BLOCK_SIZE * 2:
            size -= HEAP_BLOCK_SIZE
            if PyMem_RawRealloc(array, sizeof(Callback*) * size) != NULL:
                heap.size = size
    return rv


cdef inline int keep_top_bit(int n) nogil:
    cdef int i = 0
    while n > 1:
        n >>= 1
        i += 1
    return n << i


cdef inline void heapq_cache_friendly_heapify(HeapQueue* heap, int tail) nogil:
    cdef:
        int m = tail >> 1, mhalf = m >> 1
        int leftmost = keep_top_bit(m + 1) - 1
        int i = leftmost - 1, j

    while i >= mhalf:
        j = i
        while True:
            siftup(heap, j, tail)
            if not (j & 1):
                break
            j >>= 1
        i -= 1

    i = m - 1
    while i >= leftmost:
        j = i
        while True:
            siftup(heap, j, tail)
            if not (j & 1):
                break
            j >>= 1
        i -= 1


cdef void heapq_heapify(HeapQueue* heap) nogil:
    cdef int tail = heap.tail, i = (tail >> 1) - 1
    if tail > 2500:
        heapq_cache_friendly_heapify(heap, tail)
    else:
        while i >= 0:
            siftup(heap, i, tail)
            i -= 1
