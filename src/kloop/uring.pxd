# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef struct SubmissionQueue:
    unsigned* khead
    unsigned* ktail
    unsigned* kring_mask
    unsigned* kring_entries
    unsigned* kflags
    unsigned* kdropped
    unsigned* array
    linux.io_uring_sqe* sqes

    unsigned sqe_head
    unsigned sqe_tail

    size_t ring_size
    void* ring_ptr


cdef struct CompletionQueue:
    unsigned* khead
    unsigned* ktail
    unsigned* kring_mask
    unsigned* kring_entries
    unsigned* kflags
    unsigned* koverflow
    linux.io_uring_cqe* cqes

    size_t ring_size
    void* ring_ptr


cdef struct Ring:
    SubmissionQueue sq
    CompletionQueue cq
    unsigned flags
    int ring_fd

    unsigned features
    int enter_ring_fd
    linux.__u8 int_flags
    linux.__u8 pad[3]
    unsigned pad2


cdef struct RingCallback:
    void* data
    int res
    int (*callback)(RingCallback* cb) nogil except 0
