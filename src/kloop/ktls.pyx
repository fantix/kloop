# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


import ssl
from cpython cimport PyMem_RawMalloc, PyMem_RawFree
from libc cimport string

from .includes.openssl cimport bio, err, ssl as ssl_h
from .includes cimport pyssl


cdef object fromOpenSSLError(object err_type):
    cdef:
        unsigned long e = err.get_error()
        const char* msg = err.reason_error_string(e)
    if msg == NULL:
        return err_type()
    else:
        return err_type(msg.decode("ISO-8859-1"))


cdef int bio_write_ex(
    bio.BIO* b, const char* data, size_t datal, size_t* written
) nogil:
    with gil:
        print('bio_write', data[:datal], int(<int>data))
    bio.set_retry_write(b)
    written[0] = 0
    return 1


cdef int bio_read_ex(
    bio.BIO* b, char* data, size_t datal, size_t* readbytes
) nogil:
    with gil:
        print('bio_read', datal, int(<int>data))
    bio.set_retry_read(b)
    readbytes[0] = 0
    return 1


cdef long bio_ctrl(bio.BIO* b, int cmd, long num, void* ptr) nogil:
    cdef long ret = 0
    with gil:
        if cmd == bio.BIO_CTRL_EOF:
            print("BIO_CTRL_EOF", ret)
        elif cmd == bio.BIO_CTRL_PUSH:
            print("BIO_CTRL_PUSH", ret)
        elif cmd == bio.BIO_CTRL_FLUSH:
            ret = 1
            print('BIO_CTRL_FLUSH', ret)
        else:
            print('bio_ctrl', cmd, num)
    return ret


cdef int bio_create(bio.BIO* b) nogil:
    cdef BIO* obj = <BIO*>PyMem_RawMalloc(sizeof(BIO))
    if obj == NULL:
        return 0
    string.memset(obj, 0, sizeof(BIO))
    bio.set_data(b, <void*>obj)
    bio.set_init(b, 1)
    return 1


cdef int bio_destroy(bio.BIO* b) nogil:
    cdef void* obj = bio.get_data(b)
    if obj != NULL:
        PyMem_RawFree(obj)
    bio.set_shutdown(b, 1)
    return 1


cdef object wrap_bio(
    bio.BIO* b,
    object ssl_context,
    bint server_side=False,
    object server_hostname=None,
    object session=None,
):
    cdef pyssl.PySSLMemoryBIO* c_bio
    py_bio = ssl.MemoryBIO()
    c_bio = <pyssl.PySSLMemoryBIO*>py_bio
    c_bio.bio, b = b, c_bio.bio
    rv = ssl_context.wrap_bio(
        py_bio, py_bio, server_side, server_hostname, session
    )
    c_bio.bio, b = b, c_bio.bio
    ssl_h.set_options(
        (<pyssl.PySSLSocket*>rv._sslobj).ssl, ssl_h.OP_ENABLE_KTLS
    )
    return rv


def test():
    cdef BIO* b
    with nogil:
        b = bio.new(KTLS_BIO_METHOD)
    if b == NULL:
        raise fromOpenSSLError(RuntimeError)
    ctx = ssl.create_default_context()
    return wrap_bio(b, ctx)


cdef bio.Method* KTLS_BIO_METHOD = bio.meth_new(
    bio.get_new_index(), "kTLS BIO"
)
if not bio.meth_set_write_ex(KTLS_BIO_METHOD, bio_write_ex):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_read_ex(KTLS_BIO_METHOD, bio_read_ex):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_ctrl(KTLS_BIO_METHOD, bio_ctrl):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_create(KTLS_BIO_METHOD, bio_create):
    raise fromOpenSSLError(ImportError)
if not bio.meth_set_destroy(KTLS_BIO_METHOD, bio_destroy):
    raise fromOpenSSLError(ImportError)
