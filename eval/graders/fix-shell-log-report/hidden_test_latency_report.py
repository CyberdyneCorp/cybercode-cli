"""Hidden contract tests for latency_report.sh (see the fixture README.md).

Expected reports come from `model_report`, a direct Python transcription of the README rules,
used both for table-driven cases and for seeded random differential cases.
"""

import gzip
import os
import random
import re
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "latency_report.sh"
PROG = b"latency_report.sh"
HEADER = b"endpoint\tcount\terror%\tp50\tp95\tp99\tmax\n"
USAGE = b"usage: latency_report.sh [-n TOP] [--since TS] [--status CLASS] [--] LOG...\n"

TIMESTAMP = re.compile(rb"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")
UUID = re.compile(rb"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")


def caller_locale() -> str:
    """A non-C UTF-8 locale when the system has one, so locale-dependent sorting shows up."""
    try:
        names = subprocess.run(["locale", "-a"], capture_output=True, text=True, timeout=10).stdout.split()
    except (OSError, subprocess.SubprocessError):
        return "C"
    for name in ("en_US.UTF-8", "en_US.utf8", "C.UTF-8", "C.utf8"):
        if name in names:
            return name
    return "C"


LOCALE = caller_locale()


# ---------------------------------------------------------------- reference model


def normalize(path: bytes) -> bytes:
    path = re.split(rb"[?#]", path, maxsplit=1)[0]
    path = re.sub(rb"/+", b"/", path)
    if len(path) > 1 and path.endswith(b"/"):
        path = path[:-1]
    segments = path.split(b"/")
    for i in range(1, len(segments)):
        if re.fullmatch(rb"[0-9]+", segments[i]):
            segments[i] = b":id"
        elif UUID.fullmatch(segments[i]):
            segments[i] = b":uuid"
    return b"/".join(segments)


def parse(line: bytes):
    """Return (timestamp, endpoint, status, latency), None for a blank line, or "malformed"."""
    fields = re.split(rb"[ \t]+", line.strip(b" \t"))
    if fields == [b""]:
        return None
    if len(fields) != 5:
        return "malformed"
    ts, method, path, status, latency = fields
    ok = (
        TIMESTAMP.fullmatch(ts)
        and re.fullmatch(rb"[A-Z]+", method)
        and path.startswith(b"/")
        and re.fullmatch(rb"[1-5][0-9][0-9]", status)
        and re.fullmatch(rb"[0-9]{1,9}", latency)
    )
    if not ok:
        return "malformed"
    return ts, method + b" " + normalize(path), status, int(latency)


def nearest_rank(sorted_values, p):
    k = -(-p * len(sorted_values) // 100)
    return sorted_values[k - 1]


def error_rate(errors: int, count: int) -> bytes:
    tenths = (2000 * errors + count) // (2 * count)
    return b"%d.%d%%" % (tenths // 10, tenths % 10)


def model_report(data: bytes, top=10, since=None, status=None):
    """(stdout, malformed count) for the concatenated contents of all logs."""
    groups, malformed = {}, 0
    for line in data.split(b"\n"):
        entry = parse(line)
        if entry is None:
            continue
        if entry == "malformed":
            malformed += 1
            continue
        ts, endpoint, code, latency = entry
        if since is not None and ts < since:
            continue
        if status is not None and code[:1] != status[:1]:
            continue
        groups.setdefault(endpoint, []).append((code, latency))
    rows = []
    for endpoint, entries in groups.items():
        lat = sorted(latency for _, latency in entries)
        errors = sum(1 for code, _ in entries if code.startswith(b"5"))
        row = [endpoint, b"%d" % len(lat), error_rate(errors, len(lat))]
        row += [b"%d" % nearest_rank(lat, p) for p in (50, 95, 99)] + [b"%d" % lat[-1]]
        rows.append((len(lat), endpoint, b"\t".join(row) + b"\n"))
    rows.sort(key=lambda r: (-r[0], r[1]))
    return HEADER + b"".join(r[2] for r in rows[:top]), malformed


def malformed_stderr(n: int) -> bytes:
    return b"latency_report.sh: skipped %d malformed lines\n" % n if n else b""


def line(path=b"/x", status=200, latency=1, method=b"GET", ts=b"2024-05-01T12:00:00Z") -> bytes:
    if isinstance(path, str):
        path = path.encode()
    return b"%s %s %s %d %s\n" % (ts, method, path, status, str(latency).encode())


# ---------------------------------------------------------------- tests


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name) / "log dir"
        self.dir.mkdir()

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, name: str, data: bytes, gz: bool = False) -> str:
        path = self.dir / name
        path.write_bytes(gzip.compress(data, mtime=0) if gz else data)
        return name

    def run_script(self, *args, script=SCRIPT, cwd=None):
        env = {k: v for k, v in os.environ.items() if not k.startswith("LC_")}
        env.update(LC_ALL=LOCALE, LANG=LOCALE)
        args = [a.encode() if isinstance(a, str) else a for a in args]
        return subprocess.run(["bash", str(script), *args], cwd=cwd or self.dir, env=env,
                              stdin=subprocess.DEVNULL, capture_output=True, timeout=20)

    def assert_report(self, result, data, top=10, since=None, status=None):
        stdout, malformed = model_report(data, top, since, status)
        self.assertEqual(result.stdout, stdout)
        self.assertEqual(result.stderr, malformed_stderr(malformed))
        self.assertEqual(result.returncode, 0)

    def check(self, data: bytes, *options, top=10, since=None, status=None):
        name = self.write("access.log", data)
        self.assert_report(self.run_script(*options, name), data, top, since, status)


class NormalizationTest(Base):
    CASES = [
        ("/", "/"),
        ("//", "/"),
        ("///", "/"),
        ("/?q=1", "/"),
        ("/#top", "/"),
        ("/api/users", "/api/users"),
        ("/api/users/", "/api/users"),
        ("/api/users//", "/api/users"),
        ("/api//users/42/?x=1", "/api/users/:id"),
        ("/api/users/42?a=/b//c/", "/api/users/:id"),
        ("/files/0042#top", "/files/:id"),
        ("/files/7#frag?not=query", "/files/:id"),
        ("/files/7?q=1#frag", "/files/:id"),
        ("/a?#", "/a"),
        ("/v2/a1b2", "/v2/a1b2"),
        ("/v2/12a", "/v2/12a"),
        ("/v2/-12", "/v2/-12"),
        ("/v2/1.5", "/v2/1.5"),
        ("/1/2/3", "/:id/:id/:id"),
        ("/o/3f2504e0-4f89-11d3-9a0c-0305e82c3301/items", "/o/:uuid/items"),
        ("/o/3F2504E0-4F89-11D3-9A0C-0305E82C3301/items", "/o/:uuid/items"),
        ("/o/3F2504e0-4f89-11D3-9a0C-0305E82c3301", "/o/:uuid"),
        ("/o/12345678-1234-1234-1234-123456789012/", "/o/:uuid"),
        ("/o/3f2504e0-4f89-11d3-9a0c-0305e82c330", "/o/3f2504e0-4f89-11d3-9a0c-0305e82c330"),
        ("/o/3f2504e0-4f89-11d3-9a0c-0305e82c33011", "/o/3f2504e0-4f89-11d3-9a0c-0305e82c33011"),
        ("/o/3g2504e0-4f89-11d3-9a0c-0305e82c3301", "/o/3g2504e0-4f89-11d3-9a0c-0305e82c3301"),
        ("/o/3f2504e04f89-11d3-9a0c-0305e82c3301a", "/o/3f2504e04f89-11d3-9a0c-0305e82c3301a"),
        ("/o/x3f2504e0-4f89-11d3-9a0c-0305e82c3301", "/o/x3f2504e0-4f89-11d3-9a0c-0305e82c3301"),
        ("/Users/ME/Profile", "/Users/ME/Profile"),
        ("/café/42", "/café/:id"),
        ("/a%20b/99/", "/a%20b/:id"),
        ("/s/*/[x]/$HOME", "/s/*/[x]/$HOME"),
    ]

    def test_paths(self):
        for raw, normalized in self.CASES:
            with self.subTest(path=raw):
                name = self.write("one.log", line(raw.encode(), latency=5))
                expected = HEADER + b"GET " + normalized.encode() + b"\t1\t0.0%\t5\t5\t5\t5\n"
                result = self.run_script(name)
                self.assertEqual(result.stdout, expected)
                self.assertEqual(result.returncode, 0)

    def test_variants_group_together(self):
        data = b"".join(line(p, latency=i) for i, p in enumerate([
            "/api/users/1", "/api//users/2/", "/api/users/3?x", "/api/users/0004#f",
            "/api/users/3f2504e0-4f89-11d3-9a0c-0305e82c3301",
            "/api/users/3F2504E0-4F89-11D3-9A0C-0305E82C3301/",
        ]))
        self.check(data)


class PercentileTest(Base):
    def test_sizes(self):
        rng = random.Random(1234)
        for n in list(range(1, 22)) + [99, 100, 101, 199, 200, 201, 1000]:
            with self.subTest(n=n):
                lat = [rng.randrange(0, 100000) for _ in range(n)]
                data = b"".join(line("/p", latency=v) for v in lat)
                self.check(data)

    def test_numeric_not_lexical_order(self):
        lat = [9, 10, 100, 2, 1000, 20, 3, 300, 30, 4]
        self.check(b"".join(line("/n", latency=v) for v in lat))

    def test_explicit_values(self):
        lat = list(range(20, 0, -1))  # 1..20 shuffled backwards
        name = self.write("a.log", b"".join(line("/r", latency=v) for v in lat))
        self.assertEqual(self.run_script(name).stdout, HEADER + b"GET /r\t20\t0.0%\t10\t19\t20\t20\n")

    def test_leading_zeros(self):
        data = line("/z", latency="007") + line("/z", latency="000000000") + line("/z", latency="012345678")
        name = self.write("a.log", data)
        self.assertEqual(self.run_script(name).stdout, HEADER + b"GET /z\t3\t0.0%\t7\t12345678\t12345678\t12345678\n")

    def test_large_latencies(self):
        self.check(b"".join(line("/big", latency=v) for v in (999999999, 5, 100000000, 99999999)))


class ErrorRateTest(Base):
    CASES = [(0, 5, "0.0%"), (5, 5, "100.0%"), (1, 3, "33.3%"), (2, 3, "66.7%"), (1, 16, "6.3%"),
             (3, 16, "18.8%"), (1, 8, "12.5%"), (1, 6, "16.7%"), (1, 80, "1.3%"), (1, 200, "0.5%"),
             (1, 2000, "0.1%"), (1, 2001, "0.0%"), (7, 9, "77.8%"), (5, 32, "15.6%")]

    def test_rates(self):
        for errors, count, text in self.CASES:
            with self.subTest(errors=errors, count=count):
                codes = [503] * errors + [200] * (count - errors - 1) + [404] * (count > errors)
                data = b"".join(line("/e", status=c, latency=1) for c in codes)
                name = self.write("e.log", data)
                expected = HEADER + b"GET /e\t%d\t%s\t1\t1\t1\t1\n" % (count, text.encode())
                self.assertEqual(self.run_script(name).stdout, expected)

    def test_only_5xx_counts_as_error(self):
        codes = [100, 204, 301, 404, 499, 500, 599]
        self.check(b"".join(line("/c", status=c) for c in codes))


class SortingTest(Base):
    def test_counts_compared_numerically(self):
        data = line("/nine") * 9 + line("/ten") * 10 + line("/hundred") * 100 + line("/two") * 2
        self.check(data)

    def test_ties_by_endpoint_bytes_in_any_locale(self):
        paths = ["/a", "/B", "/_x", "/-y", "/a/b", "/a-b", "/A", "/~", "/0x"]
        data = b"".join(line(p) for p in paths) + line("/a", method=b"POST") + line("/a", method=b"DELETE")
        self.check(data)

    def test_method_is_part_of_endpoint(self):
        data = line("/r", method=b"GET") * 3 + line("/r", method=b"POST") * 3 + line("/r", method=b"PATCH") * 2
        self.check(data)

    def test_default_top_ten(self):
        data = b"".join(line("/t%02d" % i) * (1 + i % 4) for i in range(15))
        name = self.write("a.log", data)
        result = self.run_script(name)
        self.assertEqual(len(result.stdout.splitlines()), 11)
        self.assert_report(result, data)

    def test_top_option(self):
        data = b"".join(line("/t%02d" % i) * (1 + i % 5) for i in range(12))
        name = self.write("a.log", data)
        for args, top in [(["-n", "3"], 3), (["-n", "05"], 5), (["-n", "100"], 100), (["-n", "1"], 1),
                          (["-n", "2", "-n", "4"], 4)]:
            with self.subTest(args=args):
                self.assert_report(self.run_script(*args, name), data, top=top)


class FilterTest(Base):
    DATA = b"".join([
        line("/a", 200, 10, ts=b"2024-05-01T11:59:59Z"),
        line("/a", 500, 20, ts=b"2024-05-01T12:00:00Z"),
        line("/a", 503, 30, ts=b"2024-05-01T12:00:01Z"),
        line("/b", 404, 40, ts=b"2024-05-02T00:00:00Z"),
        line("/b", 201, 50, ts=b"2023-12-31T23:59:59Z"),
        line("/c", 302, 60, ts=b"2024-05-01T12:00:00Z"),
        line("/c", 101, 70, ts=b"2025-01-01T00:00:00Z"),
        b"malformed line here\n",
    ])

    def test_since(self):
        for since in [b"2024-05-01T12:00:00Z", b"2024-05-01T12:00:01Z", b"2000-01-01T00:00:00Z",
                      b"2099-01-01T00:00:00Z", b"2024-05-01T11:59:59Z"]:
            with self.subTest(since=since):
                name = self.write("f.log", self.DATA)
                self.assert_report(self.run_script("--since", since, name), self.DATA, since=since)

    def test_status(self):
        for status in ["1xx", "2xx", "3xx", "4xx", "5xx"]:
            with self.subTest(status=status):
                name = self.write("f.log", self.DATA)
                self.assert_report(self.run_script("--status", status, name), self.DATA, status=status.encode())

    def test_combined_and_repeated(self):
        name = self.write("f.log", self.DATA)
        result = self.run_script("--status", "4xx", "--since", "2024-01-01T00:00:00Z", "--status", "5xx",
                                 "-n", "1", name)
        self.assert_report(result, self.DATA, top=1, since=b"2024-01-01T00:00:00Z", status=b"5xx")

    def test_everything_filtered_prints_header(self):
        name = self.write("f.log", self.DATA)
        result = self.run_script("--since", "2099-01-01T00:00:00Z", name)
        self.assertEqual(result.stdout, HEADER)
        self.assertEqual(result.stderr, malformed_stderr(1))
        self.assertEqual(result.returncode, 0)


class MalformedTest(Base):
    BAD = [
        b"2024-05-01T12:00:00Z GET /x 200",
        b"2024-05-01T12:00:00Z GET /x 200 5 extra",
        b"2024-05-01 12:00:00Z GET /x 200 5",
        b"2024-05-01T12:00:00 GET /x 200 5",
        b"2024-5-01T12:00:00Z GET /x 200 5",
        b"2024-05-01T12:00:00z GET /x 200 5",
        b"2024-05-01T12:00:00Z get /x 200 5",
        b"2024-05-01T12:00:00Z G3T /x 200 5",
        b"2024-05-01T12:00:00Z GET x 200 5",
        b"2024-05-01T12:00:00Z GET http://h/x 200 5",
        b"2024-05-01T12:00:00Z GET /x 600 5",
        b"2024-05-01T12:00:00Z GET /x 099 5",
        b"2024-05-01T12:00:00Z GET /x 20 5",
        b"2024-05-01T12:00:00Z GET /x 2000 5",
        b"2024-05-01T12:00:00Z GET /x 2x0 5",
        b"2024-05-01T12:00:00Z GET /x 200 -5",
        b"2024-05-01T12:00:00Z GET /x 200 1.5",
        b"2024-05-01T12:00:00Z GET /x 200 1234567890",
        b"2024-05-01T12:00:00Z GET /x 200 5ms",
        b"x",
    ]

    def test_each_kind_is_malformed(self):
        for bad in self.BAD:
            with self.subTest(line=bad):
                name = self.write("m.log", bad + b"\n" + line("/ok"))
                result = self.run_script(name)
                self.assertEqual(result.stdout, HEADER + b"GET /ok\t1\t0.0%\t1\t1\t1\t1\n")
                self.assertEqual(result.stderr, b"latency_report.sh: skipped 1 malformed lines\n")
                self.assertEqual(result.returncode, 0)

    def test_blank_lines_and_whitespace(self):
        data = (b"\n   \n\t\t\n" + b"  \t2024-05-01T12:00:00Z\t\tGET   /w  200\t 9 \t\n" + b"\n"
                + b"2024-05-01T12:00:00Z GET /w 503 11")  # no trailing newline
        self.check(data)

    def test_counts_summed_across_files_and_filters(self):
        a = self.write("a.log", b"bad\n" + line("/a") + b"also bad\n")
        b = self.write("b.log.gz", b"worse\n" + line("/b", status=503), gz=True)
        result = self.run_script("--status", "5xx", a, b)
        self.assertEqual(result.stdout, HEADER + b"GET /b\t1\t100.0%\t1\t1\t1\t1\n")
        self.assertEqual(result.stderr, b"latency_report.sh: skipped 3 malformed lines\n")

    def test_only_malformed(self):
        name = self.write("m.log", b"nope\nnope nope\n")
        result = self.run_script(name)
        self.assertEqual((result.stdout, result.stderr, result.returncode),
                         (HEADER, b"latency_report.sh: skipped 2 malformed lines\n", 0))


class FilesTest(Base):
    A = line("/a", 200, 5) + line("/b", 500, 7) + line("/a", 200, 50)
    B = line("/a", 503, 9) + line("/c", 200, 1)
    C = line("/b", 200, 3) + b"junk\n"

    def test_mixed_plain_and_gzip(self):
        a = self.write("one.log", self.A)
        b = self.write("two.log.gz", self.B, gz=True)
        c = self.write("three.log.gz", self.C, gz=True)
        result = self.run_script(b, a, c)
        self.assert_report(result, self.B + self.A + self.C)

    def test_names_with_spaces_and_globs(self):
        a = self.write("access log *.log", self.A)
        b = self.write("[old] archive.log.gz", self.B, gz=True)
        self.write("access log x.log", self.C)
        self.assert_report(self.run_script(a, b), self.A + self.B)

    def test_leading_dash_names(self):
        a = self.write("-n", self.A)
        b = self.write("--status.log.gz", self.B, gz=True)
        c = self.write("-", self.C)
        self.assert_report(self.run_script("--", a, b, c), self.A + self.B + self.C)
        first = self.write("first.log", b"")
        self.assert_report(self.run_script(first, a, c), self.A + self.C)

    def test_options_after_first_log_are_files(self):
        a = self.write("a.log", self.A)
        self.write("-n", self.B)
        self.write("3", self.C)
        self.assert_report(self.run_script(a, "-n", "3"), self.A + self.B + self.C)

    def test_double_dash_after_log_is_a_file(self):
        a = self.write("a.log", self.A)
        result = self.run_script(a, "--")
        self.assertEqual((result.stdout, result.stderr, result.returncode),
                         (b"", b"latency_report.sh: cannot read --\n", 1))

    def test_same_file_twice_and_absolute_paths(self):
        a = self.write("a.log", self.A)
        b = self.write("b.log.gz", self.B, gz=True)
        result = self.run_script(str(self.dir / a), str(self.dir / b), a, b)
        self.assert_report(result, self.A + self.B + self.A + self.B)

    def test_empty_inputs(self):
        a = self.write("empty.log", b"")
        b = self.write("empty.log.gz", b"", gz=True)
        result = self.run_script(a, b)
        self.assertEqual((result.stdout, result.stderr, result.returncode), (HEADER, b"", 0))

    def test_no_trailing_newline_in_gzip(self):
        a = self.write("a.log.gz", self.A.rstrip(b"\n"), gz=True)
        b = self.write("b.log.gz", self.B.rstrip(b"\n"), gz=True)
        self.assert_report(self.run_script(a, b), self.A + self.B)

    def test_uppercase_gz_suffix_is_plain(self):
        a = self.write("a.log.GZ", self.A)
        self.assert_report(self.run_script(a), self.A)

    def test_script_path_with_spaces_and_other_cwd(self):
        tools = Path(self.tmp.name) / "my tools"
        shutil.copytree(HERE / "lib", tools / "lib")
        shutil.copy(SCRIPT, tools / "latency_report.sh")
        a = self.write("a.log", self.A)
        result = self.run_script(str(self.dir / a), script=tools / "latency_report.sh", cwd=self.tmp.name)
        self.assert_report(result, self.A)


class FileErrorTest(Base):
    def expect(self, result, message: bytes):
        self.assertEqual(result.stdout, b"")
        self.assertEqual(result.stderr, PROG + b": " + message + b"\n")
        self.assertEqual(result.returncode, 1)

    def test_missing(self):
        a = self.write("ok.log", line("/a"))
        self.expect(self.run_script("missing file.log"), b"cannot read missing file.log")
        self.expect(self.run_script(a, "missing.log"), b"cannot read missing.log")
        self.expect(self.run_script(a, "nope1.log", "nope2.log"), b"cannot read nope1.log")

    def test_directory(self):
        (self.dir / "sub dir").mkdir()
        self.expect(self.run_script("sub dir"), b"cannot read sub dir")

    def test_bad_gzip(self):
        a = self.write("ok.log", line("/a"))
        self.write("fake.log.gz", line("/b"))
        full = gzip.compress(line("/c") * 2000, mtime=0)
        (self.dir / "cut.log.gz").write_bytes(full[: len(full) // 2])
        self.expect(self.run_script(a, "fake.log.gz"), b"cannot decompress fake.log.gz")
        self.expect(self.run_script("cut.log.gz", a), b"cannot decompress cut.log.gz")
        self.expect(self.run_script("cut.log.gz", "missing.log"), b"cannot decompress cut.log.gz")
        self.expect(self.run_script("missing.log", "fake.log.gz"), b"cannot read missing.log")


class UsageTest(Base):
    CASES = [
        ([], b"no log files given"),
        (["--"], b"no log files given"),
        (["-n", "3"], b"no log files given"),
        (["-n"], b"missing value for -n"),
        (["--since"], b"missing value for --since"),
        (["--status"], b"missing value for --status"),
        (["-n", "0", "a.log"], b"invalid -n value: 0"),
        (["-n", "000", "a.log"], b"invalid -n value: 000"),
        (["-n", "-1", "a.log"], b"invalid -n value: -1"),
        (["-n", "abc", "a.log"], b"invalid -n value: abc"),
        (["-n", "2.5", "a.log"], b"invalid -n value: 2.5"),
        (["-n", "", "a.log"], b"invalid -n value: "),
        (["-n", " 3", "a.log"], b"invalid -n value:  3"),
        (["--since", "2024-05-01", "a.log"], b"invalid --since value: 2024-05-01"),
        (["--since", "2024-05-01T12:00:00", "a.log"], b"invalid --since value: 2024-05-01T12:00:00"),
        (["--since", "2024-05-01 12:00:00Z", "a.log"], b"invalid --since value: 2024-05-01 12:00:00Z"),
        (["--since", "2024-05-01T12:00:00Z ", "a.log"], b"invalid --since value: 2024-05-01T12:00:00Z "),
        (["--status", "6xx", "a.log"], b"invalid --status value: 6xx"),
        (["--status", "5XX", "a.log"], b"invalid --status value: 5XX"),
        (["--status", "5", "a.log"], b"invalid --status value: 5"),
        (["--status", "500", "a.log"], b"invalid --status value: 500"),
        (["--status", "*", "a.log"], b"invalid --status value: *"),
        (["-x", "a.log"], b"unknown option: -x"),
        (["-", "a.log"], b"unknown option: -"),
        (["-n5", "a.log"], b"unknown option: -n5"),
        (["--since=2024-05-01T12:00:00Z", "a.log"], b"unknown option: --since=2024-05-01T12:00:00Z"),
        (["--top", "3", "a.log"], b"unknown option: --top"),
        (["-n", "0", "--status", "9xx", "a.log"], b"invalid -n value: 0"),
        (["--status", "2xx", "-q", "missing.log"], b"unknown option: -q"),
        (["-n", "2", "--status"], b"missing value for --status"),
    ]

    def test_usage_errors(self):
        self.write("a.log", line("/a"))
        for args, message in self.CASES:
            with self.subTest(args=args):
                result = self.run_script(*args)
                self.assertEqual(result.stdout, b"")
                self.assertEqual(result.stderr, PROG + b": " + message + b"\n" + USAGE)
                self.assertEqual(result.returncode, 2)


# ---------------------------------------------------------------- random differential

METHODS = [b"GET", b"GET", b"GET", b"POST", b"PUT", b"DELETE"]
BASES = ["/api/users", "/api/orders", "/health", "/", "/v1/Items", "/static/app.js", "/a_b", "/A-b"]


def random_path(rng: random.Random) -> bytes:
    parts = [rng.choice(BASES)]
    for _ in range(rng.randrange(0, 3)):
        kind = rng.randrange(6)
        if kind == 0:
            parts.append(str(rng.randrange(0, 100000)).zfill(rng.choice([1, 1, 4])))
        elif kind == 1:
            u = "%08x-%04x-%04x-%04x-%012x" % tuple(rng.getrandbits(b) for b in (32, 16, 16, 16, 48))
            parts.append(rng.choice([u, u.upper(), u[:9] + u[9:].upper()]))
        elif kind == 2:
            parts.append(rng.choice(["items", "x1", "1x", "me", "deadbeef-dead-beef-dead-beefdeadbee"]))
        else:
            parts.append(rng.choice(["profile", "42", "7"]))
    path = "/".join(p.strip("/") for p in parts)
    path = "/" + path.lstrip("/")
    if rng.random() < 0.2:
        path = path.replace("/", "//", 1)
    if rng.random() < 0.2:
        path += "/"
    if rng.random() < 0.2:
        path += rng.choice(["?q=1", "?a=/1/2/", "#frag", "?x#y", "#a?b"])
    return path.encode()


def random_line(rng: random.Random) -> bytes:
    roll = rng.random()
    if roll < 0.04:
        return rng.choice([b"", b"   ", b"\t"])
    if roll < 0.1:
        return rng.choice(MalformedTest.BAD)
    ts = b"2024-05-%02dT%02d:%02d:00Z" % (rng.randrange(1, 4), rng.randrange(0, 24), rng.randrange(0, 60))
    status = rng.choice([200, 200, 200, 201, 204, 301, 404, 429, 500, 502, 503, 101])
    latency = rng.choice([rng.randrange(0, 50), rng.randrange(0, 5000), rng.randrange(0, 10 ** 7)])
    latency_text = str(latency).zfill(rng.choice([1, 1, 1, 4]))
    sep = rng.choice([b" ", b" ", b"\t", b"  "])
    fields = [ts, rng.choice(METHODS), random_path(rng), b"%d" % status, latency_text.encode()]
    return sep.join(fields)


class RandomDifferentialTest(Base):
    def test_random_logs(self):
        for seed in range(30):
            rng = random.Random(seed)
            with self.subTest(seed=seed):
                files, blobs = [], []
                for i in range(rng.randrange(1, 4)):
                    data = b"\n".join(random_line(rng) for _ in range(rng.randrange(0, 400)))
                    if data and rng.random() < 0.7:
                        data += b"\n"
                    gz = rng.random() < 0.4
                    files.append(self.write("log %d %d%s" % (seed, i, ".log.gz" if gz else ".log"), data, gz))
                    blobs.append(data if data.endswith(b"\n") or not data else data + b"\n")
                options, top, since, status = [], 10, None, None
                if rng.random() < 0.5:
                    top = rng.randrange(1, 15)
                    options += ["-n", str(top)]
                if rng.random() < 0.3:
                    since = b"2024-05-02T%02d:00:00Z" % rng.randrange(0, 24)
                    options += ["--since", since]
                if rng.random() < 0.3:
                    status = rng.choice([b"2xx", b"4xx", b"5xx"])
                    options += ["--status", status]
                if rng.random() < 0.5:
                    options.append("--")
                result = self.run_script(*options, *files)
                self.assert_report(result, b"".join(blobs), top, since, status)


class PerformanceTest(Base):
    def test_fifty_thousand_lines(self):
        rng = random.Random(99)
        data = b"\n".join(random_line(rng) for _ in range(50000)) + b"\n"
        name = self.write("big.log", data)
        start = time.monotonic()
        result = self.run_script("-n", "50", name)
        elapsed = time.monotonic() - start
        self.assert_report(result, data, top=50)
        self.assertLess(elapsed, 10.0)


if __name__ == "__main__":
    unittest.main()
