# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.

import os
import socket

from cpython cimport Py_INCREF, Py_DECREF, PyErr_SetFromErrno
from cpython cimport PyMem_RawMalloc, PyMem_RawFree
from libc cimport errno, string
from posix cimport mman

from .includes cimport barrier, libc, linux, ssl

cdef linux.__u32 SIG_SIZE = libc._NSIG // 8


class SubmissionQueueFull(Exception):
    pass


cdef class RingQueue:
    def __cinit__(self, size_t ring_size):
        self.ring_size = ring_size


cdef class SubmissionQueue(RingQueue):
    cdef init(self, linux.io_sqring_offsets sq_off):
        self.head = <unsigned*>(self.ring_ptr + sq_off.head)
        self.tail = <unsigned*>(self.ring_ptr + sq_off.tail)
        self.ring_mask = <unsigned*>(self.ring_ptr + sq_off.ring_mask)
        self.ring_entries = <unsigned*>(self.ring_ptr + sq_off.ring_entries)
        self.flags = <unsigned*>(self.ring_ptr + sq_off.flags)
        self.dropped = <unsigned*>(self.ring_ptr + sq_off.dropped)
        self.array = <unsigned*>(self.ring_ptr + sq_off.array)

    cdef linux.io_uring_sqe* next_sqe(self):
        cdef:
            unsigned int head, next
            linux.io_uring_sqe* rv
        head = barrier.io_uring_smp_load_acquire(self.head)
        next = self.sqe_tail + 1
        if next - head <= self.ring_entries[0]:
            rv = &self.sqes[self.sqe_tail & self.ring_mask[0]]
            self.sqe_tail = next
            return rv
        else:
            # TODO: IORING_ENTER_SQ_WAIT and retry
            raise SubmissionQueueFull()

    cdef unsigned flush(self):
        cdef:
            unsigned mask = self.ring_mask[0]
            unsigned tail = self.tail[0]
            unsigned to_submit = self.sqe_tail - self.sqe_head

        if to_submit:
            while to_submit:
                self.array[tail & mask] = self.sqe_head & mask
                tail += 1
                self.sqe_head += 1
                to_submit -= 1
            barrier.io_uring_smp_store_release(self.tail, tail)
        return tail - self.head[0]


cdef class CompletionQueue(RingQueue):
    cdef init(self, linux.io_cqring_offsets cq_off):
        self.head = <unsigned*>(self.ring_ptr + cq_off.head)
        self.tail = <unsigned*>(self.ring_ptr + cq_off.tail)
        self.ring_mask = <unsigned*>(self.ring_ptr + cq_off.ring_mask)
        self.ring_entries = <unsigned*>(self.ring_ptr + cq_off.ring_entries)
        self.overflow = <unsigned*>(self.ring_ptr + cq_off.overflow)
        self.cqes = <linux.io_uring_cqe*>(self.ring_ptr + cq_off.cqes)
        if cq_off.flags:
            self.flags = <unsigned*>(self.ring_ptr + cq_off.flags)

    cdef unsigned ready(self):
        return barrier.io_uring_smp_load_acquire(self.tail) - self.head[0]

    cdef inline object pop_works(self, unsigned ready):
        cdef:
            object rv = []
            Work work
            unsigned head, mask, last
            linux.io_uring_cqe* cqe
        head = self.head[0]
        mask = self.ring_mask[0]
        last = head + ready
        while head != last:
            cqe = self.cqes + (head & mask)
            work = <Work><void*>cqe.user_data
            work.res = cqe.res
            rv.append(work)
            Py_DECREF(work)
            head += 1
        barrier.io_uring_smp_store_release(self.head, self.head[0] + ready)
        return rv


cdef class Ring:
    def __cinit__(
            self,
            linux.__u32 queue_depth,
            linux.__u32 sq_thread_idle,
            object sq_thread_cpu,
    ):
        cdef:
            linux.io_uring_params params
            int fd
            size_t size
            void* ptr

        # Prepare io_uring_params
        string.memset(&params, 0, sizeof(params))
        params.flags = linux.IORING_SETUP_SQPOLL
        if sq_thread_cpu is not None:
            params.flags |= linux.IORING_SETUP_SQ_AFF
            params.sq_thread_cpu = sq_thread_cpu
        params.sq_thread_idle = sq_thread_idle

        # SYSCALL: SYS_io_uring_setup
        fd = libc.syscall(libc.SYS_io_uring_setup, queue_depth, &params)
        if fd < 0:
            PyErr_SetFromErrno(IOError)
            return
        self.fd = self.enter_fd = fd

        # Initialize 2 RingQueue and mmap the ring_ptr
        size = max(
            params.sq_off.array + params.sq_entries * sizeof(unsigned),
            params.cq_off.cqes + params.cq_entries * sizeof(linux.io_uring_cqe)
        )
        self.sq = SubmissionQueue(size)
        self.cq = CompletionQueue(size)
        ptr = mman.mmap(
            NULL,
            size,
            mman.PROT_READ | mman.PROT_WRITE,
            mman.MAP_SHARED | mman.MAP_POPULATE,
            fd,
            linux.IORING_OFF_SQ_RING,
            )
        if ptr == mman.MAP_FAILED:
            PyErr_SetFromErrno(IOError)
            return
        self.sq.ring_ptr = self.cq.ring_ptr = ptr

        # Initialize the SubmissionQueue
        self.sq.init(params.sq_off)
        size = params.sq_entries * sizeof(linux.io_uring_sqe)
        ptr = mman.mmap(
            NULL,
            size,
            mman.PROT_READ | mman.PROT_WRITE,
            mman.MAP_SHARED | mman.MAP_POPULATE,
            fd,
            linux.IORING_OFF_SQES,
            )
        if ptr == mman.MAP_FAILED:
            mman.munmap(self.sq.ring_ptr, self.sq.ring_size)
            PyErr_SetFromErrno(IOError)
            return
        self.sq.sqes = <linux.io_uring_sqe*>ptr

        # Initialize the CompletionQueue
        self.cq.init(params.cq_off)

        self.features = params.features

    def __dealloc__(self):
        if self.sq is not None:
            if self.sq.sqes != NULL:
                mman.munmap(
                    self.sq.sqes, self.sq.ring_entries[0] * sizeof(linux.io_uring_sqe)
                )
            if self.sq.ring_ptr != NULL:
                mman.munmap(self.sq.ring_ptr, self.sq.ring_size)
        if self.fd:
            os.close(self.fd)

    def submit(self, Work work):
        cdef linux.io_uring_sqe* sqe = self.sq.next_sqe()
        # print(f"submit: {work}")
        work.submit(sqe)

    def select(self, timeout):
        cdef:
            int flags = linux.IORING_ENTER_EXT_ARG, ret
            bint need_enter = False
            unsigned submit, ready
            unsigned wait_nr = 0
            linux.io_uring_getevents_arg arg
            linux.__kernel_timespec ts

        # Call enter if we have no CQE ready and timeout is not 0, or else we
        # handle the ready CQEs first.
        ready = self.cq.ready()
        if not ready and timeout is not 0:
            flags |= linux.IORING_ENTER_GETEVENTS
            if timeout is not None:
                ts.tv_sec = int(timeout)
                ts.tv_nsec = int(round((timeout - ts.tv_sec) * 1_000_000_000))
                arg.ts = <linux.__u64>&ts
            wait_nr = 1
            need_enter = True

        # Flush the submission queue, and only wakeup the SQ polling thread if
        # there is something for the kernel to handle.
        submit = self.sq.flush()
        if submit:
            barrier.io_uring_smp_mb()
            if barrier.IO_URING_READ_ONCE(
                self.sq.flags[0]
            ) & linux.IORING_SQ_NEED_WAKEUP:
                arg.ts = 0
                flags |= linux.IORING_ENTER_SQ_WAKEUP
                need_enter = True

        if need_enter:
            arg.sigmask = 0
            arg.sigmask_sz = SIG_SIZE
            # print(f"SYS_io_uring_enter(submit={submit}, wait_nr={wait_nr}, "
            #       f"flags={flags:b}, timeout={timeout})")
            with nogil:
                ret = libc.syscall(
                    libc.SYS_io_uring_enter,
                    self.enter_fd,
                    submit,
                    wait_nr,
                    flags,
                    &arg,
                    sizeof(arg),
                )
            if ret < 0:
                if errno.errno != errno.ETIME:
                    print(f"SYS_io_uring_enter(submit={submit}, wait_nr={wait_nr}, "
                          f"flags={flags:b}, timeout={timeout})")
                    PyErr_SetFromErrno(IOError)
                    return

            ready = self.cq.ready()

        if ready:
            return self.cq.pop_works(ready)
        else:
            return []


cdef class Work:
    def __init__(self, fut):
        self.fut = fut
        self.link = False
        self.res = -1

    cdef void submit(self, linux.io_uring_sqe* sqe):
        raise NotImplementedError

    cdef inline void _submit(
            self,
            int op,
            linux.io_uring_sqe * sqe,
            int fd,
            void* addr,
            unsigned len,
            linux.__u64 offset,
    ):
        string.memset(sqe, 0, sizeof(linux.io_uring_sqe))
        sqe.opcode = <linux.__u8> op
        sqe.fd = fd
        sqe.off = offset
        sqe.addr = <unsigned long> addr
        sqe.len = len
        if self.link:
            sqe.flags = linux.IOSQE_IO_LINK
        else:
            sqe.flags = 0
        sqe.user_data = <linux.__u64><void*>self
        Py_INCREF(self)

    def complete(self):
        if self.res == 0:
            self.fut.set_result(None)
        else:
            def _raise():
                errno.errno = abs(self.res)
                PyErr_SetFromErrno(IOError)
            try:
                _raise()
            except IOError as ex:
                self.fut.set_exception(ex)


cdef class ConnectWork(Work):
    def __init__(self, int fd, sockaddr, fut):
        cdef char* host
        super().__init__(fut)
        self.fd = fd
        host_str, port = sockaddr
        self.host_bytes = host_str.encode()
        host = self.host_bytes
        string.memset(&self.addr, 0, sizeof(self.addr))
        self.addr.sin_family = socket.AF_INET
        if not libc.inet_pton(socket.AF_INET, host, &self.addr.sin_addr):
            PyErr_SetFromErrno(IOError)
            return
        self.addr.sin_port = libc.htons(port)

    cdef void submit(self, linux.io_uring_sqe* sqe):
        self._submit(
            linux.IORING_OP_CONNECT,
            sqe,
            self.fd,
            &self.addr,
            0,
            sizeof(self.addr),
        )


cdef class SendWork(Work):
    def __init__(self, int fd, data, callback):
        self.fd = fd
        self.data = data
        self.data_ptr = data
        self.size = len(data)
        self.callback = callback

    cdef void submit(self, linux.io_uring_sqe* sqe):
        self._submit(linux.IORING_OP_SEND, sqe, self.fd, self.data_ptr, self.size, 0)

    def complete(self):
        self.callback(self.res)


cdef class SendMsgWork(Work):
    def __init__(self, int fd, buffers, callback):
        self.fd = fd
        self.buffers = buffers
        self.callback = callback
        self.msg.msg_iov = <libc.iovec*>PyMem_RawMalloc(
            sizeof(libc.iovec) * len(buffers)
        )
        if self.msg.msg_iov == NULL:
            raise MemoryError
        self.msg.msg_iovlen = len(buffers)
        for i, buf in enumerate(buffers):
            self.msg.msg_iov[i].iov_base = <char*>buf
            self.msg.msg_iov[i].iov_len = len(buf)

    def __dealloc__(self):
        if self.msg.msg_iov != NULL:
            PyMem_RawFree(self.msg.msg_iov)

    cdef void submit(self, linux.io_uring_sqe* sqe):
        self._submit(linux.IORING_OP_SENDMSG, sqe, self.fd, &self.msg, 1, 0)

    def complete(self):
        if self.res < 0:
            errno.errno = abs(self.res)
            PyErr_SetFromErrno(IOError)
            return
        self.callback(self.res)


cdef class RecvWork(Work):
    def __init__(self, int fd, buffer, callback):
        self.fd = fd
        self.buffer = buffer
        self.callback = callback
        self.buffer_ptr = <char*>buffer

    cdef void submit(self, linux.io_uring_sqe* sqe):
        self._submit(
            linux.IORING_OP_RECV, sqe, self.fd, self.buffer_ptr, len(self.buffer), 0
        )

    def complete(self):
        if self.res < 0:
            errno.errno = abs(self.res)
            PyErr_SetFromErrno(IOError)
            return
        self.callback(self.res)


cdef class RecvMsgWork(Work):
    def __init__(self, int fd, buffers, callback):
        cdef size_t size = libc.CMSG_SPACE(sizeof(unsigned char))
        self.fd = fd
        self.buffers = buffers
        self.callback = callback
        self.msg.msg_iov = <libc.iovec*>PyMem_RawMalloc(
            sizeof(libc.iovec) * len(buffers)
        )
        if self.msg.msg_iov == NULL:
            raise MemoryError
        self.msg.msg_iovlen = len(buffers)
        for i, buf in enumerate(buffers):
            self.msg.msg_iov[i].iov_base = <char*>buf
            self.msg.msg_iov[i].iov_len = len(buf)
        self.control_msg = bytearray(size)
        self.msg.msg_control = <char*>self.control_msg
        self.msg.msg_controllen = size

    def __dealloc__(self):
        if self.msg.msg_iov != NULL:
            PyMem_RawFree(self.msg.msg_iov)

    cdef void submit(self, linux.io_uring_sqe* sqe):
        self._submit(linux.IORING_OP_RECVMSG, sqe, self.fd, &self.msg, 1, 0)

    def complete(self):
        cdef:
            libc.cmsghdr* cmsg
            unsigned char* cmsg_data
            unsigned char record_type
        # if self.res < 0:
        #     errno.errno = abs(self.res)
        #     PyErr_SetFromErrno(IOError)
        #     return
        app_data = True
        if self.msg.msg_controllen:
            print('msg_controllen:', self.msg.msg_controllen)
            cmsg = libc.CMSG_FIRSTHDR(&self.msg)
            if cmsg.cmsg_level == libc.SOL_TLS and cmsg.cmsg_type == linux.TLS_GET_RECORD_TYPE:
                cmsg_data = libc.CMSG_DATA(cmsg)
                record_type = (<unsigned char*>cmsg_data)[0]
                if record_type != ssl.SSL3_RT_APPLICATION_DATA:
                    app_data = False
                    print(f'cmsg.len={cmsg.cmsg_len}, cmsg.level={cmsg.cmsg_level}, cmsg.type={cmsg.cmsg_type}')
                    print(f'record type: {record_type}')
                    print('flags:', self.msg.msg_flags)
        self.callback(self.res, app_data)
