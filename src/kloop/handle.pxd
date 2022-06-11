# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef unsigned char CANCELLED_MASK = 1
cdef unsigned char SCHEDULED_MASK = 1 << 1


cdef struct Callback:
    unsigned char mask
    PyObject* handle
    long long when


cdef class Handle:
    cdef:
        Callback cb
        object callback
        object args
        KLoopImpl loop
        object source_traceback
        object repr
        object context

    cdef run(self)


cdef class TimerHandle(Handle):
    cdef:
        bint scheduled
