import os
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


class KvTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = Path(self.tmp.name) / "store"

    def tearDown(self):
        self.tmp.cleanup()

    def kv(self, *args):
        env = {k: v for k, v in os.environ.items() if k != "KV_DIR"}
        return subprocess.run(["bash", str(HERE / "kv.sh"), "-d", str(self.store), *args],
                              stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=60)

    def test_set_and_get(self):
        self.assertEqual(self.kv("set", "greeting", "hello world").returncode, 0)
        result = self.kv("get", "greeting")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"hello world")

    def test_list_is_sorted(self):
        for key in ["b", "c", "a"]:
            self.kv("set", key, "1")
        result = self.kv("list")
        self.assertEqual(result.stdout, b"a\nb\nc\n")

    def test_missing_key(self):
        result = self.kv("get", "nope")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, b"")
        self.assertEqual(result.stderr, b"kv.sh: no such key: nope\n")


if __name__ == "__main__":
    unittest.main()
