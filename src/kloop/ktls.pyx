# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.

import socket
import hmac
import hashlib
from ssl import SSLWantReadError

from cpython cimport PyErr_SetFromErrno
from libc cimport string

from .includes cimport libc, linux, ssl


cdef ssl.SSL_CTX_keylog_cb_func orig_cb
cdef secrets = {}


cdef void _capture_secrets(const ssl.SSL* s, const char* line) nogil:
    if line != NULL:
        try:
            with gil:
                global secrets
                parts = line.decode("ISO-8859-1").split()
                secrets[parts[0]] = bytes.fromhex(parts[-1])
        finally:
            if orig_cb != NULL:
                orig_cb(s, line)


def do_handshake_capturing_secrets(sslobj):
    cdef:
        ssl.SSL* s = (<ssl.PySSLSocket *> sslobj._sslobj).ssl
        ssl.SSL_CTX* ctx = ssl.SSL_get_SSL_CTX(s)
    global orig_cb
    orig_cb = ssl.SSL_CTX_get_keylog_callback(ctx)
    ssl.SSL_CTX_set_keylog_callback(
        ctx, <ssl.SSL_CTX_keylog_cb_func>_capture_secrets
    )
    try:
        try:
            sslobj.do_handshake()
        except SSLWantReadError:
            success = False
        else:
            success = True
        if secrets:
            rv = dict(secrets)
            secrets.clear()
        else:
            rv = {}
        return success, rv
    finally:
        ssl.SSL_CTX_set_keylog_callback(ctx, orig_cb)


def hkdf_expand(pseudo_random_key, info=b"", length=32, hash=hashlib.sha384):
    '''
    Expand `pseudo_random_key` and `info` into a key of length `bytes` using
    HKDF's expand function based on HMAC with the provided hash (default
    SHA-512). See the HKDF draft RFC and paper for usage notes.
    '''
    # info_in = info
    # info = b'\0' + struct.pack("H", len(info)) + info + b'\0'
    # print(f'hkdf_expand info_in= label={info.hex()}')
    hash_len = hash().digest_size
    length = int(length)
    if length > 255 * hash_len:
        raise Exception("Cannot expand to more than 255 * %d = %d bytes using the specified hash function" % \
                        (hash_len, 255 * hash_len))
    blocks_needed = length // hash_len + (0 if length % hash_len == 0 else 1) # ceil
    okm = b""
    output_block = b""
    for counter in range(blocks_needed):
        output_block = hmac.new(
            pseudo_random_key,
            (output_block + info + bytearray((counter + 1,))),
            hash,
        ).digest()
        okm += output_block
    return okm[:length]


def enable_ulp(sock):
    cdef char *tls = b"tls"
    if libc.setsockopt(sock.fileno(), socket.SOL_TCP, linux.TCP_ULP, tls, 4):
        PyErr_SetFromErrno(IOError)
        return


def upgrade_aes_gcm_256(sslobj, sock, secret, sending):
    cdef:
        ssl.SSL* s = (<ssl.PySSLSocket*>sslobj._sslobj).ssl
        linux.tls12_crypto_info_aes_gcm_256 crypto_info
        char* seq

    if sending:
        # s->rlayer->write_sequence
        seq = <char*>((<void*>s) + 6112)
    else:
        # s->rlayer->read_sequence
        seq = <char*>((<void*>s) + 6104)

    # print(sslobj.cipher())

    string.memset(&crypto_info, 0, sizeof(crypto_info))
    crypto_info.info.cipher_type = linux.TLS_CIPHER_AES_GCM_256
    crypto_info.info.version = ssl.SSL_version(s)

    key = hkdf_expand(
        secret,
        b'\x00 \ttls13 key\x00',
        linux.TLS_CIPHER_AES_GCM_256_KEY_SIZE,
    )
    string.memcpy(
        crypto_info.key,
        <char*>key,
        linux.TLS_CIPHER_AES_GCM_256_KEY_SIZE,
    )
    string.memcpy(
        crypto_info.rec_seq,
        seq,
        linux.TLS_CIPHER_AES_GCM_256_REC_SEQ_SIZE,
    )
    iv = hkdf_expand(
        secret,
        b'\x00\x0c\x08tls13 iv\x00',
        linux.TLS_CIPHER_AES_GCM_256_IV_SIZE +
        linux.TLS_CIPHER_AES_GCM_256_SALT_SIZE,
    )
    string.memcpy(
        crypto_info.iv,
        <char*>iv+ ssl.EVP_GCM_TLS_FIXED_IV_LEN,
        linux.TLS_CIPHER_AES_GCM_256_IV_SIZE,
    )
    string.memcpy(
        crypto_info.salt,
        <char*>iv,
        linux.TLS_CIPHER_AES_GCM_256_SALT_SIZE,
    )
    if libc.setsockopt(
        sock.fileno(),
        libc.SOL_TLS,
        linux.TLS_TX if sending else linux.TLS_RX,
        &crypto_info,
        sizeof(crypto_info),
    ):
        PyErr_SetFromErrno(IOError)
        return
    # print(
    #     sending,
    #     "iv", crypto_info.iv[:linux.TLS_CIPHER_AES_GCM_256_IV_SIZE].hex(),
    #     "key", crypto_info.key[:linux.TLS_CIPHER_AES_GCM_256_KEY_SIZE].hex(),
    #     "salt", crypto_info.salt[:linux.TLS_CIPHER_AES_GCM_256_SALT_SIZE].hex(),
    #     "rec_seq", crypto_info.rec_seq[:linux.TLS_CIPHER_AES_GCM_256_REC_SEQ_SIZE].hex(),
    # )
