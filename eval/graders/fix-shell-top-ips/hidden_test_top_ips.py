import os
import subprocess
import tempfile
import unittest
from collections import Counter
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "top_ips.sh"


def log_line(ip, n):
    return f'{ip} - - [10/Oct/2024:13:{n % 60:02d}:00 +0000] "GET /p{n} HTTP/1.1" 200 {n * 7}\n'


def expected(lines, n=10):
    counts = Counter(line.split()[0] for line in lines if line.strip())
    ranked = sorted(counts.items(), key=lambda item: (-item[1], item[0].encode()))
    return "".join(f"{count} {ip}\n" for ip, count in ranked[:n])


class HiddenTopIpsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name) / "my logs"
        self.dir.mkdir()

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, name, lines):
        path = self.dir / name
        path.write_text("".join(lines))
        return path

    def run_script(self, *args):
        env = dict(os.environ, LC_ALL="C")
        return subprocess.run(["bash", str(SCRIPT), *map(str, args)], stdin=subprocess.DEVNULL, capture_output=True, text=True, env=env, timeout=30)

    def assert_output(self, result, stdout):
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, stdout)
        self.assertEqual(result.stderr, "")

    def test_default_top_ten_with_ties(self):
        ips = [f"10.0.{i % 3}.{i}" for i in range(14)] + ["9.9.9.9", "100.1.1.1", "2.2.2.2"]
        lines = []
        for i, ip in enumerate(ips):
            lines += [log_line(ip, i * 10 + k) for k in range(1 + i % 4)]
        lines = lines[::3] + lines[1::3] + lines[2::3]  # interleave
        path = self.write("access 1.log", lines)
        result = self.run_script(path)
        self.assert_output(result, expected(lines))
        self.assertEqual(len(result.stdout.splitlines()), 10)

    def test_ties_sorted_by_ip_bytes(self):
        lines = [log_line(ip, i) for i, ip in enumerate(["9.9.9.9", "10.0.0.2", "10.0.0.10", "192.168.0.1", "10.0.0.2", "9.9.9.9", "10.0.0.10", "192.168.0.1"])]
        path = self.write("ties.log", lines)
        self.assert_output(self.run_script(path), "2 10.0.0.10\n2 10.0.0.2\n2 192.168.0.1\n2 9.9.9.9\n")

    def test_explicit_n(self):
        lines = [log_line(ip, i) for i, ip in enumerate(["a.1", "b.2", "b.2", "c.3", "c.3", "c.3", "d.4"])]
        path = self.write("small.log", lines)
        self.assert_output(self.run_script(path, 2), "3 c.3\n2 b.2\n")
        self.assert_output(self.run_script(path, 1), "3 c.3\n")
        self.assert_output(self.run_script(path, 50), "3 c.3\n2 b.2\n1 a.1\n1 d.4\n")

    def test_large_log(self):
        lines = [log_line(f"172.16.{(i * 7919) % 31}.{(i * 104729) % 17}", i) for i in range(5000)]
        path = self.write("big.log", lines)
        self.assert_output(self.run_script(path, 25), expected(lines, 25))

    def test_blank_lines_ignored(self):
        lines = [log_line("1.1.1.1", 1), "\n", log_line("2.2.2.2", 2), "\n", log_line("1.1.1.1", 3)]
        path = self.write("blanks.log", lines)
        self.assert_output(self.run_script(path), "2 1.1.1.1\n1 2.2.2.2\n")

    def test_empty_log(self):
        path = self.write("empty.log", [])
        self.assert_output(self.run_script(path), "")

    def test_errors(self):
        path = self.write("ok.log", [log_line("1.1.1.1", 1)])
        cases = [
            (),
            (self.dir / "missing log.log",),
            (self.dir,),
            (path, "0"),
            (path, "-3"),
            (path, "abc"),
            (path, "2.5"),
        ]
        for args in cases:
            with self.subTest(args=args):
                result = self.run_script(*args)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertNotEqual(result.stderr.strip(), "")


if __name__ == "__main__":
    unittest.main()
