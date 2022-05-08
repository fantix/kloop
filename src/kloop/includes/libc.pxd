# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef extern from "sys/syscall.h" nogil:
    int SYS_io_uring_setup
    int SYS_io_uring_enter
    int SYS_io_uring_register

cdef extern from "unistd.h" nogil:
    int syscall(int number, ...)

cdef extern from "signal.h" nogil:
    int _NSIG

cdef extern from "sys/socket.h" nogil:
    enum:
        SOCK_DGRAM
        SOCK_STREAM

    ctypedef int socklen_t
    ctypedef int sa_family_t
    ctypedef int in_port_t

    int SOL_TLS

    struct sockaddr:
        sa_family_t sa_family

    struct msghdr:
        void* msg_name          # Optional address
        socklen_t msg_namelen   # Size of address
        iovec* msg_iov          # Scatter/gather array
        size_t msg_iovlen       # Number of elements in msg_iov
        void* msg_control       # ancillary data, see below
        size_t msg_controllen   # ancillary data buffer len
        int msg_flags           # flags on received message

    struct cmsghdr:
        socklen_t cmsg_len      # data byte count, including header
        int cmsg_level          # originating protocol
        int cmsg_type           # protocol-specific type

    size_t CMSG_LEN(size_t length)
    cmsghdr* CMSG_FIRSTHDR(msghdr* msgh)
    unsigned char* CMSG_DATA(cmsghdr* cmsg)
    size_t CMSG_SPACE(size_t length)

    int socket(int domain, int type, int protocol)
    int setsockopt(
        int socket,
        int level,
        int option_name,
        const void* option_value,
        socklen_t option_len,
    )
    int bind(int sockfd, const sockaddr* addr, socklen_t addrlen)


cdef extern from "arpa/inet.h" nogil:
    int inet_pton(int af, char* src, void* dst)
    int htons(short p)

cdef extern from "sys/uio.h" nogil:
    struct iovec:
        void* iov_base
        size_t iov_len


cdef extern from "<errno.h>" nogil:
    enum:
        ECANCELED
