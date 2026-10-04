import os
import subprocess
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def top_ips(*args):
    env = dict(os.environ, LC_ALL="C")
    return subprocess.run(["bash", str(HERE / "top_ips.sh"), *args], stdin=subprocess.DEVNULL, capture_output=True, text=True, env=env)


class TopIpsTest(unittest.TestCase):
    def test_counts_sample_log(self):
        result = top_ips(str(HERE / "access.log"))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.splitlines()[0], "3 10.0.0.1")

    def test_missing_file_exits_2(self):
        result = top_ips(str(HERE / "missing.log"))
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
