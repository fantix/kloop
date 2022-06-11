# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef extern from "<stdatomic.h>" nogil:
    ctypedef enum MemoryOrder "memory_order":
        relaxed "memory_order_relaxed"
        acquire "memory_order_acquire"
        release "memory_order_release"
        seq_cst "memory_order_seq_cst"

    ctypedef unsigned uint "atomic_uint"

    unsigned load_explicit "atomic_load_explicit" (
        uint* object, MemoryOrder order
    )
    void store_explicit "atomic_store_explicit" (
        uint* object, unsigned desired, MemoryOrder order
    )
    void thread_fence "atomic_thread_fence" (MemoryOrder order)
