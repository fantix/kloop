# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef const char* RESOLV_CONF = "/etc/resolv.conf"
cdef const char* HOSTS_CONF = "/etc/hosts"
cdef size_t SOCKADDR_CHUNK_SIZE = 4


cdef int resolve_cb(RingCallback* cb) nogil except 0:
    cdef:
        CResolver* r = <CResolver*>cb.data
        int rv = 1

    return rv


cdef int resolver_read_file_cb(RingCallback* cb) nogil except 0:
    cdef:
        CResolver* r = <CResolver*>cb.data
        int rv = 1
        void* ptr

    if r.hosts_conf.done_cb.res == 0 or r.resolv_conf.done_cb.res == 0:
        if cb.res < 0:
            r.resolv_conf.cancelled = r.hosts_conf.cancelled = 1
    else:
        r.res = min(r.hosts_conf.done_cb.res, r.resolv_conf.done_cb.res)
        if r.res > 0:
            r.res = resolver_init(
                r,
                r.resolv_conf.data,
                r.resolv_conf.offset,
                r.hosts_conf.data,
                r.hosts_conf.offset,
            )
        rv = queue_push(&r.loop.ready, r.cb)
        if not file_reader_done(&r.resolv_conf):
            # TODO: fd not closed?
            pass
        if not file_reader_done(&r.hosts_conf):
            # TODO: fd not closed?
            pass
    return rv


cdef extern libc.sockaddr* resolve_prep_addr(CResolve* r) nogil:
    cdef size_t l = r.result_len, size = r.result_size
    if l == size:
        size += SOCKADDR_CHUNK_SIZE
        if PyMem_RawRealloc(r.result, sizeof(libc.sockaddr) * size) == NULL:
            return NULL
        r.result_size = size
    r.result_len = l + 1
    return r.result + l


cdef extern int resolve_done_cb(CResolve* r) nogil:
    return queue_push(&r.resolver.loop.ready, r.cb)


cdef extern void resolver_set(CResolver* resolver, void* rust_resolver) nogil:
    resolver.rust_resolver = rust_resolver


cdef class Resolver:
    @staticmethod
    cdef Resolver new(KLoopImpl loop):
        cdef:
            Resolver rv = Resolver.__new__(Resolver)
            CResolver* r = &rv.resolver
        rv.loop = loop
        r.loop = &loop.loop
        r.resolv_conf.done_cb.callback = resolver_read_file_cb
        r.resolv_conf.done_cb.data = r
        r.hosts_conf.done_cb.callback = resolver_read_file_cb
        r.hosts_conf.done_cb.data = r
        return rv

    async def ensure_initialized(self):
        cdef CResolver* r

        if self.initialized:
            return
        waiter = self.waiter
        if waiter is None:
            r = &self.resolver
            waiter = self.waiter = self.loop.create_future()
            if not file_reader_start(&r.resolv_conf, r.loop, RESOLV_CONF):
                self.err_cb(ValueError("Submission queue is full!"))
            elif not file_reader_start(&r.hosts_conf, r.loop, HOSTS_CONF):
                r.resolv_conf.cancelled = 1
                self.err_cb(ValueError("Submission queue is full!"))
            else:
                self.handle = Handle(self.init_cb, (self,), self.loop, None)
                r.cb = &self.handle.cb
        await waiter

    async def reload_config(self, *, force=False):
        if self.initialized:
            waiter = self.waiter
            if waiter is None:
                waiter = self.waiter = self.loop.create_future()
                self.err_cb(NotImplementedError())
            await waiter
        else:
            await self.ensure_initialized()

    cdef init_cb(self):
        cdef int res = self.resolver.res

        if res < 0:
            try:
                errno.errno = -res
                PyErr_SetFromErrno(IOError)
            except IOError as e:
                self.waiter.set_exception(e)
        else:
            self.waiter.set_result(None)

    cdef err_cb(self, exc):
        waiter, self.waiter = self.waiter, None
        if waiter is not None:
            waiter.set_exception(exc)

    async def lookup_ip(self, host, port):
        await self.ensure_initialized()
        return await Resolve.new(self, host, port)


cdef class Resolve:
    @staticmethod
    cdef new(Resolver resolver, host, port):
        cdef:
            Resolve rv = Resolve.__new__(Resolve)
            CResolve* r = &rv.r
        rv.host = host.encode("utf-8")
        rv.waiter = resolver.loop.create_future()
        r.resolver = &resolver.resolver
        r.host = <char*>rv.host
        r.host_len = len(rv.host)
        r.port = port
        r.result = <libc.sockaddr*>PyMem_RawMalloc(
            sizeof(libc.sockaddr) * SOCKADDR_CHUNK_SIZE
        )
        if r.result == NULL:
            raise MemoryError
        r.result_size = SOCKADDR_CHUNK_SIZE
        rv.handle = Handle(rv.resolve_cb, (rv,), resolver.loop, None)
        r.cb = &rv.handle.cb
        return rv

    def __await__(self):
        cdef CResolve* r = &self.r
        resolver_lookup_ip(r.resolver.rust_resolver, r, r.host, r.host_len, r.port)
        return self.waiter.__await__()

    def __dealloc__(self):
        cdef CResolve* r = &self.r
        r.host = NULL
        r.host_len = 0
        if r.result != NULL:
            PyMem_RawFree(r.result)
            r.result = NULL
            r.result_size = 0

    cdef resolve_cb(self):
        self.waiter.set_result(self)
