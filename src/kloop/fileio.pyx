# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef size_t FILE_READ_BUF_SIZE = 4096


cdef int file_reader_openat_cb(RingCallback* cb) nogil except 0:
    cdef:
        int fd = cb.res
        FileReader* fr = <FileReader*>cb.data

    if fd < 0:
        fr.done_cb.res = fd
        return fr.done_cb.callback(&fr.done_cb)
    if fr.cancelled:
        fr.done_cb.res = -libc.ECANCELED
        return fr.done_cb.callback(&fr.done_cb)
    fr.fd = fd
    fr.data = <char*>PyMem_RawMalloc(FILE_READ_BUF_SIZE)
    if fr.data == NULL:
        fr.done_cb.res = -errno.ENOMEM
        return fr.done_cb.callback(&fr.done_cb)
    fr.size = FILE_READ_BUF_SIZE
    fr.offset = 0
    fr.ring_cb.callback = file_reader_read_cb
    if ring_sq_submit_read(
        &fr.loop.ring.sq,
        fd,
        fr.data,
        FILE_READ_BUF_SIZE,
        0,
        &fr.ring_cb,
    ):
        return 1
    else:
        fr.done_cb.res = -errno.EAGAIN
        return fr.done_cb.callback(&fr.done_cb)


cdef int file_reader_read_cb(RingCallback* cb) nogil except 0:
    cdef:
        int read = cb.res
        FileReader* fr = <FileReader*>cb.data
        size_t size = fr.size
        size_t offset = fr.offset

    if read < 0:
        fr.done_cb.res = read
        return fr.done_cb.callback(&fr.done_cb)
    if fr.cancelled:
        fr.done_cb.res = -libc.ECANCELED
        return fr.done_cb.callback(&fr.done_cb)
    offset += read
    if read > 0 and offset == size:
        size += FILE_READ_BUF_SIZE
        if PyMem_RawRealloc(fr.data, size) == NULL:
            fr.done_cb.res = -errno.ENOMEM
            return fr.done_cb.callback(&fr.done_cb)
        fr.size = size
        fr.offset = offset
        if ring_sq_submit_read(
            &fr.loop.ring.sq,
            fr.fd,
            fr.data + offset,
            FILE_READ_BUF_SIZE,
            offset,
            &fr.ring_cb,
        ):
            return 1
        else:
            fr.done_cb.res = -errno.EAGAIN
            return fr.done_cb.callback(&fr.done_cb)
    else:
        fr.done_cb.res = 1
        fr.offset = offset
        return fr.done_cb.callback(&fr.done_cb)


cdef int file_reader_start(
    FileReader* fr, Loop* loop, const char* path
) nogil:
    fr.loop = loop
    fr.done_cb.res = 0
    fr.fd = 0
    fr.data = NULL
    fr.cancelled = 0
    fr.ring_cb.callback = file_reader_openat_cb
    fr.ring_cb.data = <void*>fr
    return ring_sq_submit_openat(
        &loop.ring.sq,
        0,      # dfd
        path,   # path
        0,      # flags
        0,      # mode
        &fr.ring_cb
    )


cdef int file_reader_done(FileReader* fr) nogil:
    cdef int fd = fr.fd
    fr.done_cb.res = 0
    if fr.data != NULL:
        PyMem_RawFree(fr.data)
    fr.data = NULL
    if fd != 0:
        fr.fd = 0
        return ring_sq_submit_close(&fr.loop.ring.sq, fd, NULL)
    return 1