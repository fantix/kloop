# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.

from .includes cimport linux, libc


cdef class RingQueue:
    cdef:
        unsigned* head
        unsigned* tail
        unsigned* ring_mask
        unsigned* ring_entries
        unsigned* flags

        size_t ring_size
        void* ring_ptr


cdef class SubmissionQueue(RingQueue):
    cdef:
        unsigned* dropped
        unsigned* array
        linux.io_uring_sqe* sqes
        unsigned sqe_head
        unsigned sqe_tail

    cdef init(self, linux.io_sqring_offsets sq_off)
    cdef linux.io_uring_sqe * next_sqe(self)
    cdef unsigned flush(self)


cdef class CompletionQueue(RingQueue):
    cdef:
        unsigned* overflow
        linux.io_uring_cqe* cqes

    cdef init(self, linux.io_cqring_offsets cq_off)
    cdef unsigned ready(self)
    cdef inline object pop_works(self, unsigned ready)


cdef class Ring:
    cdef:
        SubmissionQueue sq
        CompletionQueue cq
        unsigned features
        int fd
        int enter_fd


cdef class Work:
    cdef:
        readonly object fut
        public bint link
        int res

    cdef void submit(self, linux.io_uring_sqe* sqe)

    cdef inline void _submit(
            self,
            int op,
            linux.io_uring_sqe * sqe,
            int fd,
            void * addr,
            unsigned len,
            linux.__u64 offset,
    )


cdef class ConnectWork(Work):
    cdef:
        int fd
        libc.sockaddr_in addr
        object host_bytes


cdef class SendWork(Work):
    cdef:
        int fd
        object data
        char* data_ptr
        linux.__u32 size
        object callback


cdef class SendMsgWork(Work):
    cdef:
        int fd
        list buffers
        libc.msghdr msg
        object callback


cdef class RecvWork(Work):
    cdef:
        int fd
        object buffer
        object callback
        char* buffer_ptr


cdef class RecvMsgWork(Work):
    cdef:
        int fd
        list buffers
        libc.msghdr msg
        object callback
        object control_msg
