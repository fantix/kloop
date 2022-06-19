# Copyright (c) 2022 Fantix King  https://fantix.pro
# kLoop is licensed under Mulan PSL v2.
# You can use this software according to the terms and conditions of the Mulan PSL v2.
# You may obtain a copy of Mulan PSL v2 at:
#          http://license.coscl.org.cn/MulanPSL2
# THIS SOFTWARE IS PROVIDED ON AN "AS IS" BASIS, WITHOUT WARRANTIES OF ANY KIND,
# EITHER EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO NON-INFRINGEMENT,
# MERCHANTABILITY OR FIT FOR A PARTICULAR PURPOSE.
# See the Mulan PSL v2 for more details.

import asyncio
import ssl
import time
import unittest

import kloop


class TestLoop(unittest.TestCase):
    def setUp(self):
        asyncio.set_event_loop_policy(kloop.KLoopPolicy())
        self.loop = asyncio.new_event_loop()

    def tearDown(self):
        self.loop.close()
        asyncio.set_event_loop_policy(None)

    def test_call_soon(self):
        self.loop.call_soon(self.loop.stop)
        self.loop.run_forever()

    def test_call_later(self):
        secs = 0.1
        self.loop.call_later(secs, self.loop.stop)
        start = time.monotonic()
        self.loop.run_forever()
        self.assertGreaterEqual(time.monotonic() - start, secs)

    def test_connect(self):
        ctx = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
        # ctx.check_hostname = False
        # ctx.verify_mode = ssl.CERT_NONE
        # ctx.minimum_version = ssl.TLSVersion.TLSv1_3
        host = "www.google.com"
        r, w = self.loop.run_until_complete(
            # asyncio.open_connection("127.0.0.1", 8080, ssl=ctx)
            asyncio.open_connection(host, 443, ssl=ctx)
        )
        self.loop.run_until_complete(asyncio.sleep(1))
        print('send request')
        w.write(b"GET / HTTP/1.1\r\n" +
            f"Host: {host}\r\n".encode("ISO-8859-1") +
            b"Connection: close\r\n"
            b"\r\n")
        while line := self.loop.run_until_complete(r.read()):
            print(line)
        w.close()
        self.loop.run_until_complete(w.wait_closed())
