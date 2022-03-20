# Copyright (c) 2022 Fantix King  http://fantix.pro
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
        r, w = self.loop.run_until_complete(
            asyncio.open_connection("www.google.com", 443, ssl=ctx)
        )
        w.write(b"GET / HTTP/1.1\r\n"
            b"Host: www.google.com\r\n"
            b"Connection: close\r\n"
            b"\r\n")
        while line := self.loop.run_until_complete(r.readline()):
            print(line)
        w.close()
        self.loop.run_until_complete(w.wait_closed())
