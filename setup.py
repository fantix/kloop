# Copyright (c) 2022 Fantix King  http://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


import subprocess
import sysconfig

from setuptools import setup
from Cython.Build import cythonize
from Cython.Distutils import Extension
from Cython.Distutils import build_ext


DEBUG = True
RESOLVER_LIB = (
    f"resolver/target/{'debug' if DEBUG else 'release'}/libkloop_resolver.a"
)


class build_ext_with_resolver(build_ext):
    def run(self):
        subprocess.check_call(
            ["cargo", "build"] + [] if DEBUG else ["-r"],
            cwd="resolver",
        )
        super().run()


setup(
    cmdclass={
        "build_ext": build_ext_with_resolver,
    },
    ext_modules=cythonize(
        [
            Extension(
                "kloop.loop",
                ["src/kloop/loop.pyx"],
                extra_link_args=[RESOLVER_LIB],
                depends=[RESOLVER_LIB],
            ),
            Extension(
                "kloop.ktls",
                ["src/kloop/ktls.pyx"],
                libraries=[
                    lib.strip().removeprefix("-l")
                    for lib in sysconfig.get_config_var("OPENSSL_LIBS").split()
                ],
                include_dirs=[
                    d.strip().removeprefix("-I")
                    for d in sysconfig.get_config_var(
                        "OPENSSL_INCLUDES"
                    ).split()
                ],
                library_dirs=[
                    d.strip().removeprefix("-L")
                    for d in sysconfig.get_config_var(
                        "OPENSSL_LDFLAGS"
                    ).split()
                    if d.strip().startswith("-L")
                ],
                extra_link_args=[
                    d.strip()
                    for d in sysconfig.get_config_var(
                        "OPENSSL_LDFLAGS"
                    ).split()
                    if not d.strip().startswith("-L")
                ],
                runtime_library_dirs=(lambda x: [x] if x else [])(
                    sysconfig.get_config_var("OPENSSL_RPATH")
                ),
            ),
        ],
        language_level="3",
    ),
)
