# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.


import os
import subprocess
import sysconfig

from setuptools import setup

from Cython.Build import cythonize
from Cython.Distutils import build_ext
from Cython.Distutils import Extension
from distutils import dir_util
from distutils import log
from distutils.command.clean import clean


class build_ext_with_resolver(build_ext):
    def finalize_options(self):
        self.set_undefined_options(
            "build",
            ("debug", "debug"),
            ("force", "force"),
        )
        if self.debug is None:
            self.debug = os.getenv("KLOOP_DEBUG", "0") == "1"
        if self.force is None:
            self.force = os.getenv("KLOOP_FORCE", "0") == "1"

        for ext in self.distribution.ext_modules:
            if self.debug:
                if "-O0" not in ext.extra_compile_args:
                    ext.extra_compile_args.append("-O0")
            if ext.name == "kloop.loop":
                resolver = (
                    f"resolver/target/{'debug' if self.debug else 'release'}"
                    f"/libkloop_resolver.a"
                )
                if resolver not in ext.extra_link_args:
                    ext.extra_link_args.append(resolver)
                if resolver not in ext.depends:
                    ext.depends.append(resolver)

        self.distribution.ext_modules = cythonize(
            self.distribution.ext_modules, language_level="3",
        )

        super().finalize_options()

    def run(self):
        if self.force:
            cmd = ["cargo", "clean", "-p", "resolver"]
            if not self.debug:
                cmd.append("-r")
            self.announce(f"Running: {cmd}", log.INFO)
            subprocess.check_call(cmd, cwd="resolver")

        cmd = ["cargo", "build"]
        if not self.debug:
            cmd.append("-r")
        self.announce(f"Running: {cmd}", log.INFO)
        subprocess.check_call(cmd, cwd="resolver")
        super().run()


class clean_with_resolver(clean):
    def run(self):
        super().run()
        for d in self.distribution.package_dir.values():
            self._clean_dir(d)
        cmd = ["cargo", "clean"]
        if not self.all:
            cmd.extend(["-p", "resolver"])
        self.announce(f"Running: {cmd}", log.INFO)
        if not self.dry_run:
            subprocess.check_call(cmd, cwd="resolver")

    def _clean_dir(self, path):
        for f in os.listdir(path):
            name, ext = os.path.splitext(f)
            real_f = os.path.join(path, f)
            is_dir = os.path.isdir(real_f) and not os.path.islink(real_f)
            if name == "__pycache__" or ext in {".egg-info", ".so", ".c"}:
                if is_dir:
                    dir_util.remove_tree(real_f, dry_run=self.dry_run)
                else:
                    self.announce(f"removing {real_f!r}", log.INFO)
                    if not self.dry_run:
                        os.remove(real_f)
            elif is_dir:
                self._clean_dir(real_f)


setup(
    cmdclass={
        "build_ext": build_ext_with_resolver,
        "clean": clean_with_resolver,
    },
    ext_modules=[
        Extension(
            "kloop.loop",
            ["src/kloop/loop.pyx"],
        ),
        Extension(
            "kloop.tls",
            ["src/kloop/tls.pyx"],
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
)
