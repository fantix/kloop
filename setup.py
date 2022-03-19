# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


from Cython.Build import cythonize
from Cython.Distutils import Extension
from setuptools import setup

setup(
    ext_modules=cythonize(
        [
            Extension("kloop.uring", ["src/kloop/uring.pyx"]),
            Extension("kloop.ktls", ["src/kloop/ktls.pyx"]),
        ],
        language_level="3",
    )
)
