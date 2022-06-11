# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef int QUEUE_BLOCK_SIZE = 1024


cdef int queue_init(Queue* queue) nogil except 0:
    queue.array = <Callback**>PyMem_RawMalloc(
        sizeof(Callback*) * QUEUE_BLOCK_SIZE
    )
    if queue.array == NULL:
        with gil:
            raise MemoryError
    queue.head = -1
    queue.tail = 0
    queue.size = QUEUE_BLOCK_SIZE
    return 1


cdef void queue_uninit(Queue* queue) nogil:
    cdef:
        int i = queue.head, size = queue.size, tail = queue.tail
        Callback** array = queue.array

    if array == NULL:
        return
    if i >= 0:
        with gil:
            while True:
                Py_DECREF(<object>array[i].handle)
                i = (i + 1) % size
                if i == tail:
                    break
    PyMem_RawFree(array)
    queue.array = NULL


cdef queue_push_py(Queue* queue, Handle handle):
    cdef Callback* callback = &handle.cb
    Py_INCREF(handle)
    with nogil:
        queue_push(queue, callback)


cdef int queue_push(Queue* queue, Callback* callback) nogil except 0:
    cdef:
        Callback** orig = queue.array
        Callback** array = orig
        int size = queue.size, head = queue.head, tail = queue.tail

    if head == tail:
        if head == 0:
            tail = size
            size += QUEUE_BLOCK_SIZE
            array = <Callback**>PyMem_RawRealloc(
                orig, sizeof(Callback*) * size
            )
            if array == NULL:
                with gil:
                    raise MemoryError
        else:
            tail = size + QUEUE_BLOCK_SIZE
            array = <Callback**>PyMem_RawMalloc(sizeof(Callback*) * tail)
            if array == NULL:
                with gil:
                    raise MemoryError
            queue.array = array
            string.memcpy(
                array, orig + head, sizeof(Callback*) * (size - head)
            )
            string.memcpy(array + size - head, orig, sizeof(Callback*) * head)
            size, tail = tail, size
            queue.head = head = 0
            PyMem_RawFree(orig)
        queue.size = size
    elif head < 0:
        queue.head = head = 0
    array[tail] = callback
    queue.tail = (tail + 1) % size
    return 1


cdef Handle queue_pop_py(Queue* queue):
    cdef:
        Handle handle
        Callback* callback

    with nogil:
        callback = queue_pop(queue)
    if callback == NULL:
        return None
    else:
        handle = <Handle>callback.handle
        Py_DECREF(handle)
        return handle


cdef Callback* queue_pop(Queue* queue) nogil:
    cdef:
        int size = queue.size, head = queue.head, tail = queue.tail
        Callback* rv
        Callback** orig = queue.array
        Callback** array = orig

    if head < 0:
        return NULL
    rv = array[head]
    queue.head = head = (head + 1) % size
    if head == tail:
        queue.head = -1
        queue.tail = 0
        if size > QUEUE_BLOCK_SIZE:
            size = QUEUE_BLOCK_SIZE
            if PyMem_RawRealloc(
                array, sizeof(Callback*) * size
            ) != NULL:
                queue.size = size
    elif (head - tail) % size >= QUEUE_BLOCK_SIZE * 2:
        if head < tail:
            size -= QUEUE_BLOCK_SIZE
            if tail > size:
                tail -= head
                string.memmove(array, array + head, sizeof(Callback*) * tail)
                queue.tail = tail
                queue.head = 0
            if PyMem_RawRealloc(
                array, sizeof(Callback*) * size
            ) != NULL:
                queue.size = size
                queue.tail = tail % size
        else:
            array = <Callback**>PyMem_RawMalloc(
                sizeof(Callback*) * (size - QUEUE_BLOCK_SIZE)
            )
            if array != NULL:
                string.memcpy(
                    array, orig + head, sizeof(Callback*) * (size - head)
                )
                string.memcpy(
                    array + size - head, orig, sizeof(Callback*) * tail
                )
                queue.array = array
                queue.head = 0
                queue.tail = (tail - head) % size
                queue.size = size - QUEUE_BLOCK_SIZE
                PyMem_RawFree(orig)
    return rv


cdef int queue_size(Queue* queue) nogil:
    cdef int size = queue.size, head = queue.head, tail = queue.tail
    if head < 0:
        return 0
    elif head == tail:
        return size
    else:
        return (tail - head) % size
