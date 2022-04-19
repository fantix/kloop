cdef extern from "sys/syscall.h" nogil:
    int SYS_io_uring_setup
    int SYS_io_uring_enter
    int SYS_io_uring_register

cdef extern from "unistd.h" nogil:
    int syscall(int number, ...)

cdef extern from "signal.h" nogil:
    int _NSIG

cdef extern from "sys/socket.h" nogil:
    ctypedef int socklen_t
    int SOL_TLS

    int setsockopt(int socket, int level, int option_name,
                   const void *option_value, socklen_t option_len);

    struct in_addr:
        pass

    struct sockaddr_in:
        int sin_family
        int sin_port
        in_addr sin_addr

    struct msghdr:
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


cdef extern from "arpa/inet.h" nogil:
    int inet_pton(int af, char* src, void* dst)
    int htons(short p)

cdef extern from "sys/uio.h" nogil:
    struct iovec:
        void* iov_base
        size_t iov_len
