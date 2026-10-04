import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent

EXPECTED_SAMPLE = (
    "endpoint\tcount\terror%\tp50\tp95\tp99\tmax\n"
    "GET /api/users/:id\t5\t20.0%\t95\t120\t120\t120\n"
    "GET /health\t4\t0.0%\t2\t4\t4\t4\n"
    "GET /api/orders/:uuid\t3\t0.0%\t64\t71\t71\t71\n"
    "POST /api/orders\t2\t50.0%\t340\t1200\t1200\t1200\n"
)


def report(*args):
    env = dict(os.environ, LC_ALL="C")
    return subprocess.run(["bash", str(HERE / "latency_report.sh"), *args], stdin=subprocess.DEVNULL,
                          capture_output=True, text=True, env=env, timeout=60)


class LatencyReportTest(unittest.TestCase):
    def test_sample_report(self):
        result = report(str(HERE / "sample" / "access.log"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, EXPECTED_SAMPLE)
        self.assertEqual(result.stderr, "")

    def test_log_path_with_spaces(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "access log.log"
            shutil.copy(HERE / "sample" / "access.log", path)
            result = report(str(path))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, EXPECTED_SAMPLE)

    def test_missing_file_exits_1(self):
        result = report(str(HERE / "sample" / "missing.log"))
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("cannot read", result.stderr)


if __name__ == "__main__":
    unittest.main()
