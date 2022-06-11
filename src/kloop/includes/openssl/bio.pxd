# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


cdef extern from "openssl/bio.h" nogil:
    enum BIOCtrl:
        BIO_CTRL_RESET         # 1 opt - rewind/zero etc
        BIO_CTRL_EOF           # 2 opt - are we at the eof
        BIO_CTRL_INFO          # 3 opt - extra tit-bits
        BIO_CTRL_SET           # 4 man - set the 'IO' type
        BIO_CTRL_GET           # 5 man - get the 'IO' type
        BIO_CTRL_PUSH          # 6 opt - internal, used to signify change
        BIO_CTRL_POP           # 7 opt - internal, used to signify change
        BIO_CTRL_GET_CLOSE     # 8 man - set the 'close' on free
        BIO_CTRL_SET_CLOSE     # 9 man - set the 'close' on free
        BIO_CTRL_PENDING       # 10 opt - is their more data buffered
        BIO_CTRL_FLUSH         # 11 opt - 'flush' buffered output
        BIO_CTRL_DUP           # 12 man - extra stuff for 'duped' BIO
        BIO_CTRL_WPENDING      # 13 opt - number of bytes still to write
        BIO_CTRL_SET_CALLBACK  # 14 opt - set callback function
        BIO_CTRL_GET_CALLBACK  # 15 opt - set callback function

    ctypedef struct Method "BIO_METHOD":
        pass

    ctypedef struct BIO:
        pass

    int get_new_index "BIO_get_new_index" ()

    Method* meth_new "BIO_meth_new" (int type, const char* name)

    int meth_set_write_ex "BIO_meth_set_write_ex" (
        Method* biom,
        int (*bwrite)(BIO*, const char*, size_t, size_t*),
    )
    int meth_set_write "BIO_meth_set_write" (
        Method* biom,
        int (*write)(BIO*, const char*, int),
    )

    int meth_set_read_ex "BIO_meth_set_read_ex" (
        Method* biom,
        int (*bread)(BIO*, char*, size_t, size_t*),
    )
    int meth_set_read "BIO_meth_set_read"(
        Method* biom,
        int (*read)(BIO*, char*, int),
    )

    int meth_set_ctrl "BIO_meth_set_ctrl" (
        Method* biom, long (*ctrl)(BIO*, int, long, void*)
    )

    int meth_set_create "BIO_meth_set_create" (
        Method* biom,
        int (*create)(BIO*),
    )

    int meth_set_destroy "BIO_meth_set_destroy" (
        Method* biom,
        int (*destroy)(BIO*),
    )

    ctypedef int info_cb "BIO_info_cb" (BIO*, int, int)
    int meth_set_callback_ctrl "BIO_meth_set_callback_ctrl" (
        Method* biom,
        long (*callback_ctrl)(BIO*, int, info_cb*),
    )

    BIO* new "BIO_new" (const Method* type)
    int up_ref "BIO_up_ref" (BIO* a)
    int free "BIO_free" (BIO* a)

    void set_data "BIO_set_data" (BIO* a, void* ptr)
    void* get_data "BIO_get_data" (BIO* a)
    void set_init "BIO_set_init" (BIO* a, int init)
    void set_shutdown "BIO_set_shutdown" (BIO* a, int shut)

    void set_retry_read "BIO_set_retry_read" (BIO *b)
    void set_retry_write "BIO_set_retry_write" (BIO *b)
