import unittest

from kloop import uring, ktls


class TestLoop(unittest.TestCase):
    def test_loop(self):
        self.assertIsNotNone(uring)
        self.assertIsNotNone(ktls)
