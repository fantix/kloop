# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


from .. cimport linux


cdef extern from "openssl/ssl.h" nogil:
    ctypedef struct SSL:
        pass

    int SSL3_RT_APPLICATION_DATA
    int OP_ENABLE_KTLS "SSL_OP_ENABLE_KTLS"
    int set_options "SSL_set_options" (SSL* ssl, int options)


cdef extern from *:
    """
    typedef struct {
        union {
            struct tls12_crypto_info_aes_gcm_128 gcm128;
            struct tls12_crypto_info_aes_gcm_256 gcm256;
            struct tls12_crypto_info_aes_ccm_128 ccm128;
            struct tls12_crypto_info_chacha20_poly1305 chacha20poly1305;
        };
        size_t tls_crypto_info_len;
    } ktls_crypto_info_t;
    """

    ctypedef struct ktls_crypto_info_t:
        size_t tls_crypto_info_len
