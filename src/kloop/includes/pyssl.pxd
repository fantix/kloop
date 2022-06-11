# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


from .openssl.ssl cimport SSL
from .openssl.bio cimport BIO

cdef extern from *:
    """
    typedef struct {
        PyObject_HEAD
        PyObject *Socket; /* weakref to socket on which we're layered */
        SSL *ssl;
    } PySSLSocket;

    typedef struct {
        PyObject_HEAD
        BIO *bio;
        int eof_written;
    } PySSLMemoryBIO;
    """

    ctypedef struct PySSLSocket:
        SSL* ssl

    ctypedef struct PySSLMemoryBIO:
        BIO* bio
