# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.

cdef extern from "openssl/ssl.h" nogil:
    int EVP_GCM_TLS_FIXED_IV_LEN

    ctypedef struct SSL:
        pass

    ctypedef struct SSL_CTX:
        pass

    int SSL_version(const SSL *s)
    ctypedef void(*SSL_CTX_keylog_cb_func)(SSL *ssl, char *line)
    void SSL_CTX_set_keylog_callback(SSL_CTX* ctx, SSL_CTX_keylog_cb_func cb)
    SSL_CTX_keylog_cb_func SSL_CTX_get_keylog_callback(SSL_CTX* ctx)
    SSL_CTX* SSL_get_SSL_CTX(SSL* ssl)


cdef extern from "includes/ssl.h" nogil:
    ctypedef struct PySSLSocket:
        SSL *ssl
