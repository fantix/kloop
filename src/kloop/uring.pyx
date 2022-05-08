# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef linux.__u32 SIG_SIZE = libc._NSIG // 8


cdef int ring_init(
    Ring* ring,
    linux.__u32 queue_depth,
    linux.io_uring_params* params
) nogil except 0:
    # SYSCALL: SYS_io_uring_setup
    ring.ring_fd = ring.enter_ring_fd = libc.syscall(
        libc.SYS_io_uring_setup, queue_depth, params
    )
    if ring.ring_fd < 0:
        with gil:
            PyErr_SetFromErrno(IOError)
        return 0

    # mmap the ring_ptr
    ring.sq.ring_size = ring.cq.ring_size = max(
        params.sq_off.array + params.sq_entries * sizeof(unsigned),
        params.cq_off.cqes + params.cq_entries * sizeof(linux.io_uring_cqe)
    )
    ring.sq.ring_ptr = ring.cq.ring_ptr = mman.mmap(
        NULL,
        ring.sq.ring_size,
        mman.PROT_READ | mman.PROT_WRITE,
        mman.MAP_SHARED | mman.MAP_POPULATE,
        ring.ring_fd,
        linux.IORING_OFF_SQ_RING,
    )
    if ring.sq.ring_ptr == mman.MAP_FAILED:
        with gil:
            PyErr_SetFromErrno(IOError)
        return 0

    # Initialize the SubmissionQueue
    ring.sq.khead = <unsigned*>(ring.sq.ring_ptr + params.sq_off.head)
    ring.sq.ktail = <unsigned*>(ring.sq.ring_ptr + params.sq_off.tail)
    ring.sq.kring_mask = <unsigned*>(ring.sq.ring_ptr + params.sq_off.ring_mask)
    ring.sq.kring_entries = <unsigned*>(ring.sq.ring_ptr + params.sq_off.ring_entries)
    ring.sq.kflags = <unsigned*>(ring.sq.ring_ptr + params.sq_off.flags)
    ring.sq.kdropped = <unsigned*>(ring.sq.ring_ptr + params.sq_off.dropped)
    ring.sq.array = <unsigned*>(ring.sq.ring_ptr + params.sq_off.array)
    ring.sq.sqes = <linux.io_uring_sqe*>mman.mmap(
        NULL,
        params.sq_entries * sizeof(linux.io_uring_sqe),
        mman.PROT_READ | mman.PROT_WRITE,
        mman.MAP_SHARED | mman.MAP_POPULATE,
        ring.ring_fd,
        linux.IORING_OFF_SQES,
    )
    if ring.sq.sqes == mman.MAP_FAILED:
        mman.munmap(ring.sq.ring_ptr, ring.sq.ring_size)
        with gil:
            PyErr_SetFromErrno(IOError)
        return 0

    # Initialize the CompletionQueue
    ring.cq.khead = <unsigned*>(ring.cq.ring_ptr + params.cq_off.head)
    ring.cq.ktail = <unsigned*>(ring.cq.ring_ptr + params.cq_off.tail)
    ring.cq.kring_mask = <unsigned*>(ring.cq.ring_ptr + params.cq_off.ring_mask)
    ring.cq.kring_entries = <unsigned*>(ring.cq.ring_ptr + params.cq_off.ring_entries)
    ring.cq.koverflow = <unsigned*>(ring.cq.ring_ptr + params.cq_off.overflow)
    ring.cq.cqes = <linux.io_uring_cqe*>(ring.cq.ring_ptr + params.cq_off.cqes)
    if params.cq_off.flags:
        ring.cq.kflags = <unsigned*>(ring.cq.ring_ptr + params.cq_off.flags)

    return 1


cdef int ring_uninit(Ring* ring) nogil except 0:
    if ring.sq.sqes != NULL:
        mman.munmap(
            ring.sq.sqes,
            ring.sq.kring_entries[0] * sizeof(linux.io_uring_sqe),
        )
    if ring.sq.ring_ptr != NULL:
        mman.munmap(ring.sq.ring_ptr, ring.sq.ring_size)
    if ring.ring_fd:
        if unistd.close(ring.ring_fd) != 0:
            with gil:
                PyErr_SetFromErrno(IOError)
            return 0
    return 1


cdef inline unsigned ring_sq_flush(SubmissionQueue* sq) nogil:
    cdef:
        unsigned mask = sq.kring_mask[0]
        unsigned tail = sq.ktail[0]
        unsigned to_submit = sq.sqe_tail - sq.sqe_head

    if to_submit:
        while to_submit:
            sq.array[tail & mask] = sq.sqe_head & mask
            tail += 1
            sq.sqe_head += 1
            to_submit -= 1
        barrier.io_uring_smp_store_release(sq.ktail, tail)
    return tail - sq.khead[0]


cdef int ring_select(Ring* ring, long long timeout) nogil except -1:
    cdef:
        int flags = linux.IORING_ENTER_EXT_ARG
        bint need_enter = 0
        unsigned submit, ready
        unsigned wait_nr = 0
        linux.io_uring_getevents_arg arg
        linux.__kernel_timespec ts
        CompletionQueue* cq = &ring.cq
        SubmissionQueue* sq = &ring.sq

    # Call enter if we have no CQE ready and timeout is not 0, or else we
    # handle the ready CQEs first.
    ready = barrier.io_uring_smp_load_acquire(cq.ktail) - cq.khead[0]
    if not ready and timeout != 0:
        flags |= linux.IORING_ENTER_GETEVENTS
        if timeout > 0:
            ts.tv_sec = timeout // SECOND_NS
            ts.tv_nsec = timeout % SECOND_NS
            arg.ts = <linux.__u64> &ts
        wait_nr = 1
        need_enter = 1

    # Flush the submission queue, and only wakeup the SQ polling thread if
    # there is something for the kernel to handle.
    submit = ring_sq_flush(sq)
    if submit:
        barrier.io_uring_smp_mb()
        if barrier.IO_URING_READ_ONCE(
            sq.kflags[0]
        ) & linux.IORING_SQ_NEED_WAKEUP:
            arg.ts = 0
            flags |= linux.IORING_ENTER_SQ_WAKEUP
            need_enter = 1

    if need_enter:
        arg.sigmask = 0
        arg.sigmask_sz = SIG_SIZE
        if libc.syscall(
            libc.SYS_io_uring_enter,
            ring.enter_ring_fd,
            submit,
            wait_nr,
            flags,
            &arg,
            sizeof(arg),
        ) < 0:
            if errno.errno != errno.ETIME:
                with gil:
                    PyErr_SetFromErrno(IOError)
                    return -1

        ready = barrier.io_uring_smp_load_acquire(cq.ktail) - cq.khead[0]

    return ready


cdef inline void ring_cq_pop(CompletionQueue* cq, RingCallback** callback) nogil:
    cdef:
        unsigned head
        linux.io_uring_cqe* cqe
        RingCallback* ret
    head = cq.khead[0]
    cqe = cq.cqes + (head & cq.kring_mask[0])
    ret = <RingCallback*>cqe.user_data
    if ret != NULL:
        ret.res = cqe.res
        callback[0] = ret
    barrier.io_uring_smp_store_release(cq.khead, head + 1)


cdef inline linux.io_uring_sqe* ring_sq_submit(
    SubmissionQueue* sq,
    linux.__u8 op,
    int fd,
    unsigned long addr,
    unsigned len,
    linux.__u64 offset,
    bint link,
    RingCallback* callback,
) nogil:
    cdef:
        unsigned int head, next
        linux.io_uring_sqe* sqe
    head = barrier.io_uring_smp_load_acquire(sq.khead)
    next = sq.sqe_tail + 1
    if next - head <= sq.kring_entries[0]:
        sqe = &sq.sqes[sq.sqe_tail & sq.kring_mask[0]]
        sq.sqe_tail = next

        string.memset(sqe, 0, sizeof(linux.io_uring_sqe))
        sqe.opcode = op
        sqe.fd = fd
        sqe.off = offset
        sqe.addr = addr
        sqe.len = len
        if link:
            sqe.flags = linux.IOSQE_IO_LINK
        sqe.user_data = <linux.__u64>callback
        return sqe
    else:
        return NULL


cdef int ring_sq_submit_connect(
    SubmissionQueue* sq, int fd, libc.sockaddr* addr, RingCallback* callback
) nogil:
    return 1 if ring_sq_submit(
        sq,
        linux.IORING_OP_CONNECT,
        fd,
        <unsigned long>addr,
        0,
        sizeof(addr[0]),
        0,
        callback,
    ) else 0


cdef int ring_sq_submit_openat(
    SubmissionQueue* sq,
    int dfd,
    const char* path,
    int flags,
    mode_t mode,
    RingCallback* callback,
) nogil:
    cdef linux.io_uring_sqe* sqe = ring_sq_submit(
        sq,
        linux.IORING_OP_OPENAT,
        dfd,
        <unsigned long>path,
        mode,
        0,
        0,
        callback,
    )
    if sqe == NULL:
        return 0
    else:
        sqe.open_flags = flags
        return 1


cdef int ring_sq_submit_read(
    SubmissionQueue* sq,
    int fd,
    char* buf,
    unsigned nbytes,
    linux.__u64 offset,
    RingCallback* callback,
) nogil:
    return 1 if ring_sq_submit(
        sq,
        linux.IORING_OP_READ,
        fd,
        <unsigned long>buf,
        nbytes,
        offset,
        0,
        callback,
    ) else 0


cdef int ring_sq_submit_close(
    SubmissionQueue* sq,
    int fd,
    RingCallback * callback,
) nogil:
    return 1 if ring_sq_submit(
        sq,
        linux.IORING_OP_CLOSE,
        fd,
        0,
        0,
        0,
        0,
        callback,
    ) else 0

cdef int ring_sq_submit_sendmsg(
    SubmissionQueue* sq,
    int fd,
    const libc.msghdr *msg,
    RingCallback* callback,
) nogil:
    return 1 if ring_sq_submit(
        sq,
        linux.IORING_OP_SENDMSG,
        fd,
        <unsigned long>msg,
        1,
        0,
        0,
        callback,
    ) else 0
