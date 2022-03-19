# kLoop

kLoop is an implementation of the Python
[asyncio](https://docs.python.org/3/library/asyncio.html) event loop written
in [Cython](https://cython.org/), using
[io_uring](https://unixism.net/loti/what_is_io_uring.html) and
[kTLS](https://www.kernel.org/doc/html/latest/networking/tls-offload.html)
features of the Linux kernel, open-sourced and released
under the [MulanPSL - 2.0 license](http://license.coscl.org.cn/MulanPSL2).

**⚠️WARNING: THIS PROJECT IS IN PROOF-OF-CONCEPT STAGE!⚠️**


## Requirements

* Python >= 3.8
* Linux >= 5.11 (enable ktls with `modprobe ktls`)
* OpenSSL 1.1.1 (3.0 is unnecessary)

