# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef extern from "linux/fs.h" nogil:
    ctypedef int __kernel_rwf_t


cdef extern from "linux/types.h" nogil:
    ctypedef int __u8
    ctypedef int __u16
    ctypedef int __u64
    ctypedef int __u32
    ctypedef int __s32
    ctypedef int __kernel_time64_t


cdef extern from "linux/time_types.h" nogil:
    struct __kernel_timespec:
        __kernel_time64_t tv_sec
        long long tv_nsec


cdef extern from "linux/tcp.h" nogil:
    int TCP_ULP


cdef extern from "linux/tls.h" nogil:
    int TLS_GET_RECORD_TYPE

    __u16 TLS_CIPHER_AES_GCM_256
    int TLS_CIPHER_AES_GCM_256_IV_SIZE
    int TLS_CIPHER_AES_GCM_256_SALT_SIZE
    int TLS_CIPHER_AES_GCM_256_KEY_SIZE
    int TLS_CIPHER_AES_GCM_256_REC_SEQ_SIZE
    int TLS_TX
    int TLS_RX

    struct tls_crypto_info:
        __u16 version
        __u16 cipher_type

    struct tls12_crypto_info_aes_gcm_256:
        tls_crypto_info info
        unsigned char* iv
        unsigned char* key
        unsigned char* salt
        unsigned char* rec_seq


cdef extern from "linux/io_uring.h" nogil:
    unsigned IORING_SETUP_SQPOLL
    unsigned IORING_SETUP_SQ_AFF

    unsigned IORING_ENTER_GETEVENTS
    unsigned IORING_ENTER_SQ_WAKEUP
    unsigned IORING_ENTER_EXT_ARG

    unsigned IORING_SQ_NEED_WAKEUP
    unsigned IORING_SQ_CQ_OVERFLOW

    unsigned long long IORING_OFF_SQ_RING
    unsigned long long IORING_OFF_SQES

    unsigned IORING_TIMEOUT_ABS

    unsigned IOSQE_IO_LINK

    enum Operation:
        IORING_OP_NOP
        IORING_OP_CONNECT
        IORING_OP_SEND
        IORING_OP_SENDMSG
        IORING_OP_RECV
        IORING_OP_RECVMSG
        IORING_OP_OPENAT
        IORING_OP_READ
        IORING_OP_CLOSE

    struct io_sqring_offsets:
        __u32 head
        __u32 tail
        __u32 ring_mask
        __u32 ring_entries
        __u32 flags
        __u32 dropped
        __u32 array

    struct io_cqring_offsets:
        __u32 head
        __u32 tail
        __u32 ring_mask
        __u32 ring_entries
        __u32 overflow
        __u32 cqes
        __u32 flags

    struct io_uring_params:
        __u32 flags
        __u32 sq_thread_cpu
        __u32 sq_thread_idle

        # written by the kernel:
        __u32 sq_entries
        __u32 cq_entries
        __u32 features
        __u32 resv[4]
        io_sqring_offsets sq_off
        io_cqring_offsets cq_off

    struct io_uring_sqe:
        __u8 opcode         # type of operation for this sqe
        __s32 fd            # file descriptor to do IO on
        __u64 off           # offset into file
        __u64 addr          # pointer to buffer or iovecs
        __u32 len           # buffer size or number of iovecs
        __u64 user_data     # data to be passed back at completion time
        __u8 flags          # IOSQE_ flags
        __u32 open_flags

    struct io_uring_cqe:
        __u64 user_data     # data to be passed back at completion time
        __s32 res           # result code for this event

    struct io_uring_getevents_arg:
        __u64 sigmask
        __u32 sigmask_sz
        __u64 ts
