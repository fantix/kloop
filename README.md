# kLoop

[English](README.en.md)

kLoop 是一个 Python
[asyncio](https://docs.python.org/3/library/asyncio.html) event loop 的实现，
主要用 [Cython](https://cython.org/) 编写，重点使用了 Linux 内核的
[io_uring](https://unixism.net/loti/what_is_io_uring.html) 和
[kTLS](https://www.kernel.org/doc/html/latest/networking/tls-offload.html)
功能，故称作 k(ernel)Loop。

您可在[木兰宽松许可证, 第2版](http://license.coscl.org.cn/MulanPSL2)的范围内使用
kLoop 的源代码。

**⚠️警告：项目仍在概念验证当中，满地都是坑！⚠️**


## 环境需求

* Python >= 3.8
* Linux >= 5.11 (用 `modprobe ktls` 命令来启用 kTLS 模块)
* OpenSSL >= 3.0
