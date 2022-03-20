/* Copied from liburing: src/include/liburing/barrier.h */

#include <stdatomic.h>

#define IO_URING_WRITE_ONCE(var, val)                               \
        atomic_store_explicit((_Atomic __typeof__(var) *)&(var),    \
                              (val), memory_order_relaxed)
#define IO_URING_READ_ONCE(var)                                     \
        atomic_load_explicit((_Atomic __typeof__(var) *)&(var),     \
                             memory_order_relaxed)

#define io_uring_smp_store_release(p, v)                            \
        atomic_store_explicit((_Atomic __typeof__(*(p)) *)(p), (v), \
                              memory_order_release)
#define io_uring_smp_load_acquire(p)                                \
        atomic_load_explicit((_Atomic __typeof__(*(p)) *)(p),       \
                             memory_order_acquire)

#define io_uring_smp_mb()                                           \
        atomic_thread_fence(memory_order_seq_cst)
