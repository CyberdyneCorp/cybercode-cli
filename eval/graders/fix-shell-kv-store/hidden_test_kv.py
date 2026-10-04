"""Hidden contract tests for kv.sh (see the fixture README.md).

Every call passes raw bytes straight to `bash kv.sh` through subprocess (no shell in between).
`Model` is a direct Python transcription of the README, used for table-driven expectations and
for seeded random differential sequences.
"""

import json
import os
import random
import subprocess
import tempfile
import time
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "kv.sh"
MAX_INT = 10 ** 18 - 1
SYNOPSIS = {
    b"set": b"set KEY VALUE", b"get": b"get KEY", b"del": b"del KEY", b"list": b"list [PREFIX]",
    b"export": b"export [--json]", b"import": b"import FILE", b"incr": b"incr KEY [N]",
}


def caller_locale() -> str:
    """A non-C UTF-8 locale when the system has one, so locale-dependent behavior shows up."""
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


def enc(key: bytes) -> bytes:
    out = bytearray()
    for i, b in enumerate(key):
        ch = bytes([b])
        if (b < 128 and (ch.islower() or ch.isdigit())) or ch in (b"_", b"-") or (ch == b"." and i > 0):
            out += ch
        else:
            out += b"%%%02X" % b
    return bytes(out)


def esc_text(s: bytes) -> bytes:
    return s.replace(b"\\", b"\\\\").replace(b"\t", b"\\t").replace(b"\n", b"\\n")


def esc_json(s: bytes) -> bytes:
    out = bytearray()
    for b in s:
        special = {0x22: b'\\"', 0x5C: b"\\\\", 0x0A: b"\\n", 0x09: b"\\t", 0x0D: b"\\r"}
        if b in special:
            out += special[b]
        elif b < 0x20 or b == 0x7F:
            out += b"\\u%04x" % b
        else:
            out.append(b)
    return bytes(out)


def is_integer(text: bytes) -> bool:
    digits = text[1:] if text.startswith(b"-") else text
    return 1 <= len(digits) <= 18 and all(48 <= b <= 57 for b in digits)


def err(message: bytes, status: int = 1):
    return status, b"", b"kv.sh: " + message + b"\n"


class Model:
    """In-memory transcription of the README for commands other than import."""

    def __init__(self):
        self.data: dict[bytes, bytes] = {}

    def run(self, args: list[bytes]):
        if not args:
            return err(b"missing command", 2)
        command, rest = args[0], args[1:]
        if command not in SYNOPSIS:
            return err(b"unknown command: " + command, 2)
        counts = {b"set": (2,), b"get": (1,), b"del": (1,), b"list": (0, 1), b"export": (0, 1),
                  b"incr": (1, 2)}
        bad_export = command == b"export" and rest not in ([], [b"--json"])
        if len(rest) not in counts[command] or bad_export:
            return err(b"usage: kv.sh [-d DIR] " + SYNOPSIS[command], 2)
        if command in (b"set", b"get", b"del", b"incr") and (not rest[0] or b"\n" in rest[0]):
            return err(b"invalid key", 2)
        return getattr(self, "do_" + command.decode())(*rest)

    def do_set(self, key, value):
        self.data[key] = value
        return 0, b"", b""

    def do_get(self, key):
        if key not in self.data:
            return err(b"no such key: " + key)
        return 0, self.data[key], b""

    def do_del(self, key):
        if key not in self.data:
            return err(b"no such key: " + key)
        del self.data[key]
        return 0, b"", b""

    def do_list(self, prefix=b""):
        return 0, b"".join(k + b"\n" for k in sorted(self.data) if k.startswith(prefix)), b""

    def do_export(self, flag=None):
        return 0, self.export(json_format=flag is not None), b""

    def export(self, json_format=False) -> bytes:
        items = sorted(self.data.items())
        if json_format:
            return b"{" + b",".join(b'"%s":"%s"' % (esc_json(k), esc_json(v)) for k, v in items) + b"}\n"
        return b"".join(esc_text(k) + b"\t" + esc_text(v) + b"\n" for k, v in items)

    def do_incr(self, key, step=b"1"):
        if not is_integer(step):
            return err(b"invalid increment: " + step, 2)
        current = self.data.get(key, b"0")
        if not is_integer(current):
            return err(b"not an integer: " + key)
        result = int(current) + int(step)
        if abs(result) > MAX_INT:
            return err(b"integer overflow: " + key)
        self.data[key] = b"%d" % result
        return 0, b"%d\n" % result, b""


# ---------------------------------------------------------------- fixtures

NASTY_KEYS = [
    b"plain", b"with space", b"  lead and trail  ", b"user/1", b"a/b/c/", b"/abs", b"*", b"a*b", b"?",
    b"[a]", b"[!a]", b"-n", b"-e", b"--", b"-", b".", b"..", b"../../escape", b".env", b"a.b", b"v1.2",
    b"100%", b"%41", b"%2F", b"back\\slash", b"\\n", b"tab\there", b"cr\rkey", b"quote\"s", b"it's",
    b"$HOME", b"`id`", b"$(id)", b"caf\xc3\xa9", b"\xe2\x9c\x93 check", b"\xff\xfe", b"semi;colon", b"a=b",
    b"~", b"_under", b"MiXeD", b"0", b"007", b"key\x01ctl", b"x" * 80,
]

NASTY_VALUES = [
    b"", b"\n", b"\n\n", b"line\n", b"two\n\n", b"\nlead", b"a\nb\nc", b"-n", b"-e", b"-E", b"-n -e x",
    b"%s", b"%d%%", b"%b", b"\\", b"\\n", b"\\c", b"\\0101", b"*", b"?", b"[a-z]", b"$HOME", b"$(id)",
    b"`id`", b"  spaced  ", b"\t", b"a\tb\t", b"\r\n", b"crlf\r\n\r\n", bytes(range(1, 32)) + b"\x7f",
    b"\xff\xfe\x80\x9b\x85", b"caf\xc3\xa9", b'{"json": "yes"}', b"'single'", b"x" * 20000,
    b"line\n" * 300,
]


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.store = self.root / "my store"

    def tearDown(self):
        self.tmp.cleanup()

    def kv(self, *args, store=True, stdin=None, env=None, cwd=None, timeout=30):
        argv = [a if isinstance(a, bytes) else str(a).encode() for a in args]
        if store:
            argv = [b"-d", bytes(self.store)] + argv
        environ = {k: v for k, v in os.environ.items()
                   if not k.startswith("LC_") and k not in ("KV_DIR", "KV_LOCK_TIMEOUT")}
        # A short default lock timeout keeps a broken lock from stalling the whole suite.
        environ.update(LC_ALL=LOCALE, LANG=LOCALE, KV_LOCK_TIMEOUT="3")
        environ.update(env or {})
        return subprocess.run([b"bash", bytes(SCRIPT), *argv], input=stdin,
                              stdin=None if stdin is not None else subprocess.DEVNULL,
                              capture_output=True, env=environ, cwd=cwd or self.root, timeout=timeout)

    def assert_result(self, result, status, stdout=b"", stderr=b""):
        self.assertEqual((result.returncode, result.stdout, result.stderr), (status, stdout, stderr))

    def assert_ok(self, result, stdout=b""):
        self.assert_result(result, 0, stdout)

    def assert_store(self, expected: dict, store: Path | None = None):
        store = store or self.store
        names = sorted(os.listdir(bytes(store))) if store.exists() else []
        files = {n: (store / os.fsdecode(n)).read_bytes() for n in names if not n.startswith(b".")}
        self.assertEqual(files, {enc(k): v for k, v in expected.items()})
        self.assertNotIn(b".lock", names)

    def write(self, name: str, data: bytes) -> Path:
        path = self.root / name
        path.write_bytes(data)
        return path


class EncodingTest(Base):
    def test_examples_from_readme(self):
        cases = {b"user/1": b"user%2F1", b".env": b"%2Eenv", b"..": b"%2E.", b"a b*": b"a%20b%2A",
                 b"v1.2": b"v1.2", b"Key": b"%4Bey", b"100%": b"100%25", b"\xc3\xa9": b"%C3%A9", b"-n": b"-n"}
        for key, name in cases.items():
            self.assertEqual(enc(key), name)
            self.assert_ok(self.kv("set", key, b"v"))
        self.assert_store({k: b"v" for k in cases})

    def test_nasty_keys(self):
        expected = {}
        for i, key in enumerate(NASTY_KEYS):
            with self.subTest(key=key):
                value = b"value %d\n" % i
                self.assert_ok(self.kv("set", key, value))
                self.assert_ok(self.kv("get", key), value)
                expected[key] = value
        self.assert_store(expected)
        self.assert_ok(self.kv("list"), b"".join(k + b"\n" for k in sorted(expected)))
        self.assertFalse((self.root / "escape").exists())

    def test_overwrite_and_delete(self):
        for key in [b"user/1", b"..", b"-n", b"a b", b"*"]:
            with self.subTest(key=key):
                self.assert_ok(self.kv("set", key, b"one"))
                self.assert_ok(self.kv("set", key, b"two"))
                self.assert_ok(self.kv("get", key), b"two")
                self.assert_ok(self.kv("del", key))
                self.assert_result(self.kv("get", key), 1, stderr=b"kv.sh: no such key: " + key + b"\n")
        self.assert_store({})

    def test_glob_key_does_not_touch_others(self):
        self.assert_ok(self.kv("set", "a1", "x"))
        self.assert_ok(self.kv("set", "a2", "y"))
        self.assert_ok(self.kv("set", "a*", "star"))
        self.assert_ok(self.kv("del", "a*"))
        self.assert_store({b"a1": b"x", b"a2": b"y"})
        self.assert_result(self.kv("del", "a?"), 1, stderr=b"kv.sh: no such key: a?\n")


class ValuesTest(Base):
    def test_values_round_trip_exactly(self):
        for i, value in enumerate(NASTY_VALUES):
            with self.subTest(value=value[:40]):
                key = b"k%d" % i
                self.assert_ok(self.kv("set", key, value))
                self.assert_ok(self.kv("get", key), value)
                self.assertEqual((self.store / key.decode()).read_bytes(), value)

    def test_values_that_look_like_options(self):
        self.assert_ok(self.kv("set", "-n", "-e"))
        self.assert_ok(self.kv("set", "--", "--"))
        self.assert_ok(self.kv("set", "-d", "-d"))
        self.assert_ok(self.kv("get", "-n"), b"-e")
        self.assert_ok(self.kv("get", "--"), b"--")
        self.assert_ok(self.kv("get", "-d"), b"-d")
        self.assert_store({b"-n": b"-e", b"--": b"--", b"-d": b"-d"})


class ListTest(Base):
    KEYS = [b"a", b"B", b"_x", b"-y", b"a/b", b"a b", b"a*", b"a*b", b"[a]", b"\\", b"\\x", b"\xc3\xa9",
            b"Z", b"z", b"10", b"9", b".hidden", b"..", b"a-b", b"a.b", b"abc"]

    def setUp(self):
        super().setUp()
        for key in self.KEYS:
            self.assert_ok(self.kv("set", key, b"v"))

    def expect(self, prefix):
        return b"".join(k + b"\n" for k in sorted(self.KEYS) if k.startswith(prefix))

    def test_sorted_by_bytes(self):
        self.assert_ok(self.kv("list"), self.expect(b""))
        self.assert_ok(self.kv("list", ""), self.expect(b""))

    def test_prefixes_are_literal(self):
        for prefix in [b"a", b"a*", b"*", b"[a]", b"[", b"\\", b"?", b"a?", b".", b"..", b"-", b"\xc3",
                       b"nothing", b"a b", b"a/"]:
            with self.subTest(prefix=prefix):
                self.assert_ok(self.kv("list", prefix), self.expect(prefix))

    def test_too_many_arguments(self):
        self.assert_result(self.kv("list", "a", "b"), 2, stderr=b"kv.sh: usage: kv.sh [-d DIR] list [PREFIX]\n")


class ExportTest(Base):
    def fill(self, pairs):
        model = Model()
        for key, value in pairs:
            self.assert_ok(self.kv("set", key, value))
            model.data[key] = value
        return model

    def test_text_export(self):
        model = self.fill(zip(NASTY_KEYS, NASTY_VALUES))
        self.assert_ok(self.kv("export"), model.export())

    def test_json_export(self):
        model = self.fill(zip(NASTY_KEYS, NASTY_VALUES))
        result = self.kv("export", "--json")
        self.assert_ok(result, model.export(json_format=True))

    def test_json_escapes(self):
        model = self.fill([(b"ctl", bytes(range(1, 32)) + b"\x7f"), (b"q", b'"\\/'), (b"utf", b"caf\xc3\xa9 \xe2\x9c\x93"),
                           (b"k\"\\\t", b"x"), (b"hi", b"\x80\x85\x9b\xa0\xff")])
        result = self.kv("export", "--json")
        self.assert_ok(result, model.export(json_format=True))
        self.assertIn(b"\\u001b", result.stdout)
        self.assertIn(b"\\u007f", result.stdout)
        valid = {k: v for k, v in model.data.items() if k != b"hi"}
        self.kv("del", "hi")
        parsed = json.loads(self.kv("export", "--json").stdout.decode())
        self.assertEqual(parsed, {k.decode(): v.decode() for k, v in valid.items()})

    def test_empty_and_missing_store(self):
        self.assert_ok(self.kv("export"), b"")
        self.assert_ok(self.kv("export", "--json"), b"{}\n")
        self.assert_ok(self.kv("list"), b"")
        self.assert_result(self.kv("get", "x"), 1, stderr=b"kv.sh: no such key: x\n")
        self.assertFalse(self.store.exists())
        self.assert_ok(self.kv("set", "x", "1"))
        self.assert_ok(self.kv("del", "x"))
        self.assert_ok(self.kv("export", "--json"), b"{}\n")

    def test_bad_arguments(self):
        for args in (["--csv"], ["--json", "x"], ["json"], [""]):
            with self.subTest(args=args):
                self.assert_result(self.kv("export", *args), 2,
                                   stderr=b"kv.sh: usage: kv.sh [-d DIR] export [--json]\n")

    def test_round_trip_through_import(self):
        model = self.fill(zip(NASTY_KEYS, reversed(NASTY_VALUES)))
        dump = self.kv("export").stdout
        other = self.root / "copy"
        self.assert_ok(self.kv("-d", bytes(other), "import", "-", store=False, stdin=dump))
        self.assert_store(model.data, other)


class ImportTest(Base):
    def test_basic_file(self):
        path = self.write("in put.tsv", b"a\t1\nb b\tline\\none\\n\n\nc\\\\d\ttab\\there\r\n" b"a\tlast wins")
        self.assert_ok(self.kv("set", "keep", "me"))
        self.assert_ok(self.kv("import", path))
        self.assert_store({b"a": b"last wins", b"b b": b"line\none\n", b"c\\d": b"tab\there\r", b"keep": b"me"})

    def test_stdin_and_empty_value(self):
        self.assert_ok(self.kv("import", "-", stdin=b"-n\t\n--\t-e\n\\\\\t\\\\\\\\\n"))
        self.assert_store({b"-n": b"", b"--": b"-e", b"\\": b"\\\\"})

    def test_empty_input(self):
        self.assert_ok(self.kv("import", "-", stdin=b""))
        self.assert_ok(self.kv("import", "-", stdin=b"\n\n"))
        self.assert_ok(self.kv("list"), b"")

    def test_errors_are_all_or_nothing(self):
        cases = [
            (b"a\t1\nnotab\n", b"import: line 2: expected one tab"),
            (b"a\t1\n\nb\t2\tthree\n", b"import: line 3: expected one tab"),
            (b"a\t1\n\\x\t2\n", b"import: line 2: bad escape"),
            (b"a\t1\nk\tv\\\n", b"import: line 2: bad escape"),
            (b"k\\\tv\n", b"import: line 1: bad escape"),
            (b"k\tv\\r\n", b"import: line 1: bad escape"),
            (b"\t1\n", b"import: line 1: invalid key"),
            (b"a\t1\nx\\ny\t2\n", b"import: line 2: invalid key"),
            (b"\\q\tx\ty\n", b"import: line 1: expected one tab"),
            (b"\\\t\\q\n", b"import: line 1: bad escape"),
            (b"\t\\q\n", b"import: line 1: bad escape"),
            (b"a\t1\r\nb\t2\n\n\n c\n", b"import: line 5: expected one tab"),
            (b"a\t1\nlast\\", b"import: line 2: expected one tab"),
        ]
        self.assert_ok(self.kv("set", "a", "orig"))
        for data, message in cases:
            with self.subTest(data=data):
                path = self.write("bad.tsv", data)
                self.assert_result(self.kv("import", path), 1, stderr=b"kv.sh: " + message + b"\n")
                self.assert_store({b"a": b"orig"})

    def test_unreadable_file(self):
        self.assert_result(self.kv("import", "no such.tsv"), 1, stderr=b"kv.sh: cannot read no such.tsv\n")
        (self.root / "adir").mkdir()
        self.assert_result(self.kv("import", "adir"), 1, stderr=b"kv.sh: cannot read adir\n")
        self.assert_result(self.kv("import"), 2, stderr=b"kv.sh: usage: kv.sh [-d DIR] import FILE\n")


class IncrTest(Base):
    def test_sequences(self):
        model = Model()
        steps = [("c", None), ("c", None), ("c", "5"), ("c", "-10"), ("c", "007"), ("c", "-08"), ("c", "0"),
                 ("c", "-0"), ("n", "-3"), ("n", "-000"), ("z", "999999999999999998"), ("z", "1"),
                 ("w", "-999999999999999999"), ("lead", "0010")]
        for key, step in steps:
            args = ["incr", key] + ([step] if step is not None else [])
            with self.subTest(args=args):
                expected = model.run([a.encode() for a in args])
                result = self.kv(*args)
                self.assert_result(result, *expected)
        self.assert_store(model.data)

    def test_existing_values(self):
        cases = [
            (b"41", b"1", 0, b"42"), (b"007", b"1", 0, b"8"), (b"-08", b"8", 0, b"0"), (b"-1", b"1", 0, b"0"),
            (b"-000", b"0", 0, b"0"), (b"0", b"-5", 0, b"-5"), (b"999999999999999999", b"-1", 0, b"999999999999999998"),
            (b"5\n", b"1", 1, b"not an integer: k"), (b" 5", b"1", 1, b"not an integer: k"),
            (b"+5", b"1", 1, b"not an integer: k"), (b"1.0", b"1", 1, b"not an integer: k"),
            (b"", b"1", 1, b"not an integer: k"), (b"0x10", b"1", 1, b"not an integer: k"),
            (b"-", b"1", 1, b"not an integer: k"), (b"1-", b"1", 1, b"not an integer: k"),
            (b"1234567890123456789", b"1", 1, b"not an integer: k"), (b"--1", b"1", 1, b"not an integer: k"),
            (b"999999999999999999", b"1", 1, b"integer overflow: k"),
            (b"-999999999999999999", b"-1", 1, b"integer overflow: k"),
            (b"500000000000000000", b"500000000000000000", 1, b"integer overflow: k"),
        ]
        for value, step, status, text in cases:
            with self.subTest(value=value, step=step):
                self.assert_ok(self.kv("set", "k", value))
                result = self.kv("incr", "k", step)
                if status == 0:
                    self.assert_ok(result, text + b"\n")
                    self.assert_store({b"k": text})
                else:
                    self.assert_result(result, status, stderr=b"kv.sh: " + text + b"\n")
                    self.assert_store({b"k": value})

    def test_invalid_increments(self):
        self.assert_ok(self.kv("set", "k", "1"))
        for step in [b"", b"+1", b"1e3", b"--1", b"abc", b" 1", b"1 ", b"1234567890123456789", b"-", b"0x1", b"1.5"]:
            with self.subTest(step=step):
                self.assert_result(self.kv("incr", "k", step), 2, stderr=b"kv.sh: invalid increment: " + step + b"\n")
        self.assert_store({b"k": b"1"})

    def test_nasty_keys(self):
        for key in [b"a b", b"-n", b"..", b"x/y", b"*"]:
            with self.subTest(key=key):
                self.assert_ok(self.kv("incr", key, "2"), b"2\n")
                self.assert_ok(self.kv("get", key), b"2")


class ErrorsTest(Base):
    def test_usage_and_dispatch(self):
        usage = lambda cmd: b"kv.sh: usage: kv.sh [-d DIR] " + SYNOPSIS[cmd] + b"\n"
        cases = [
            ([], 2, b"kv.sh: missing command\n"),
            (["frob"], 2, b"kv.sh: unknown command: frob\n"),
            (["SET", "a", "b"], 2, b"kv.sh: unknown command: SET\n"),
            (["set"], 2, usage(b"set")),
            (["set", "a"], 2, usage(b"set")),
            (["set", "a", "b", "c"], 2, usage(b"set")),
            (["set", "", "b", "c"], 2, usage(b"set")),
            (["get"], 2, usage(b"get")),
            (["get", "a", "b"], 2, usage(b"get")),
            (["del"], 2, usage(b"del")),
            (["incr"], 2, usage(b"incr")),
            (["incr", "a", "1", "2"], 2, usage(b"incr")),
            (["set", "", "v"], 2, b"kv.sh: invalid key\n"),
            (["set", "a\nb", "v"], 2, b"kv.sh: invalid key\n"),
            (["set", "a\n", "v"], 2, b"kv.sh: invalid key\n"),
            (["get", ""], 2, b"kv.sh: invalid key\n"),
            (["del", "\n"], 2, b"kv.sh: invalid key\n"),
            (["incr", "", "x"], 2, b"kv.sh: invalid key\n"),
            (["incr", "k", "x"], 2, b"kv.sh: invalid increment: x\n"),
        ]
        for args, status, stderr in cases:
            with self.subTest(args=args):
                self.assert_result(self.kv(*args), status, stderr=stderr)
        self.assertFalse(self.store.exists())

    def test_global_options(self):
        cases = [
            (["-x", "list"], b"kv.sh: unknown option: -x\n"),
            (["--", "list"], b"kv.sh: unknown option: --\n"),
            (["-d"], b"kv.sh: missing value for -d\n"),
            (["-d", "", "list"], b"kv.sh: missing value for -d\n"),
            (["-d", "s", "-v", "get", "a"], b"kv.sh: unknown option: -v\n"),
            (["-d", "s"], b"kv.sh: missing command\n"),
            (["-d", "s", "nope"], b"kv.sh: unknown command: nope\n"),
        ]
        for args, stderr in cases:
            with self.subTest(args=args):
                self.assert_result(self.kv(*args, store=False), 2, stderr=stderr)

    def test_missing_keys(self):
        self.assert_ok(self.kv("set", "a", "1"))
        for key in [b"b", b"a ", b"A", b"a*", b"-n", b"..", b"%61"]:
            with self.subTest(key=key):
                self.assert_result(self.kv("get", key), 1, stderr=b"kv.sh: no such key: " + key + b"\n")
                self.assert_result(self.kv("del", key), 1, stderr=b"kv.sh: no such key: " + key + b"\n")
        self.assert_store({b"a": b"1"})


class StoreLocationTest(Base):
    def test_default_and_env(self):
        self.assert_ok(self.kv("set", "k", "default", store=False))
        self.assertEqual((self.root / ".kv" / "k").read_bytes(), b"default")
        env_dir = self.root / "from env"
        self.assert_ok(self.kv("set", "k", "env", store=False, env={"KV_DIR": str(env_dir)}))
        self.assertEqual((env_dir / "k").read_bytes(), b"env")
        self.assert_ok(self.kv("set", "k", "empty env", store=False, env={"KV_DIR": ""}))
        self.assertEqual((self.root / ".kv" / "k").read_bytes(), b"empty env")
        flag_dir = self.root / "flag"
        self.assert_ok(self.kv("-d", "ignored", "-d", str(flag_dir), "set", "k", "flag", store=False,
                               env={"KV_DIR": str(env_dir)}))
        self.assertEqual((flag_dir / "k").read_bytes(), b"flag")
        self.assertFalse((self.root / "ignored").exists())
        self.assert_ok(self.kv("get", "k", store=False, env={"KV_DIR": str(env_dir)}), b"env")

    def test_odd_directories(self):
        for name in ["-store", "nested/deeper/dir", "sp ace/*", "--"]:
            with self.subTest(dir=name):
                self.assert_ok(self.kv("-d", name, "set", "-n", "v\n", store=False))
                self.assert_ok(self.kv("-d", name, "get", "-n", store=False), b"v\n")
                self.assert_ok(self.kv("-d", name, "list", store=False), b"-n\n")
                self.assertEqual((self.root / name / "-n").read_bytes(), b"v\n")
                self.assert_ok(self.kv("-d", name, "del", "-n", store=False))

    def test_reserved_files_are_not_keys(self):
        self.assert_ok(self.kv("set", "a", "1"))
        (self.store / ".tmp.123").write_bytes(b"junk")
        (self.store / ".other").write_bytes(b"junk")
        self.assert_ok(self.kv("list"), b"a\n")
        self.assert_ok(self.kv("export", "--json"), b'{"a":"1"}\n')


SLOW_LOCK = {"KV_LOCK_TIMEOUT": "60"}


class LockTest(Base):
    def make_lock(self, pid: int | None):
        lock = self.store / ".lock"
        lock.mkdir(parents=True)
        if pid is not None:
            (lock / "pid").write_text(f"{pid}\n")
        return lock

    def dead_pid(self) -> int:
        proc = subprocess.Popen(["true"])
        proc.wait()
        return proc.pid

    def test_stale_lock_is_reclaimed(self):
        self.make_lock(self.dead_pid())
        start = time.monotonic()
        self.assert_ok(self.kv("set", "a", "1", env={"KV_LOCK_TIMEOUT": "5"}))
        self.assertLess(time.monotonic() - start, 4)
        self.assert_ok(self.kv("incr", "n", env={"KV_LOCK_TIMEOUT": "5"}), b"1\n")
        self.assert_store({b"a": b"1", b"n": b"1"})

    def test_live_lock_times_out(self):
        self.assert_ok(self.kv("set", "a", "1"))
        lock = self.make_lock(os.getpid())
        for args in (["set", "a", "2"], ["del", "a"], ["incr", "n"], ["import", "-"]):
            with self.subTest(args=args):
                start = time.monotonic()
                result = self.kv(*args, env={"KV_LOCK_TIMEOUT": "1"}, stdin=b"a\t3\n")
                elapsed = time.monotonic() - start
                self.assert_result(result, 3, stderr=b"kv.sh: store is locked\n")
                self.assertGreater(elapsed, 0.8)
                self.assertLess(elapsed, 6)
                self.assertTrue(lock.is_dir())
                self.assert_ok(self.kv("get", "a"), b"1")
                self.assert_ok(self.kv("list"), b"a\n")

    def test_lock_without_pid_is_waited_on(self):
        lock = self.make_lock(None)
        result = self.kv("set", "a", "1", env={"KV_LOCK_TIMEOUT": "1"})
        self.assert_result(result, 3, stderr=b"kv.sh: store is locked\n")
        self.assertTrue(lock.is_dir())
        self.assertEqual(sorted(p.name for p in self.store.iterdir() if not p.name.startswith(".")), [])

    def test_lock_released_after_errors(self):
        self.assert_ok(self.kv("set", "a", "text"))
        self.assert_result(self.kv("incr", "a"), 1, stderr=b"kv.sh: not an integer: a\n")
        self.assert_result(self.kv("del", "missing"), 1, stderr=b"kv.sh: no such key: missing\n")
        self.assert_result(self.kv("import", "-", stdin=b"bad\n"), 1, stderr=b"kv.sh: import: line 1: expected one tab\n")
        self.assert_result(self.kv("incr", "big", "999999999999999999"), 0, stdout=b"999999999999999999\n")
        self.assert_result(self.kv("incr", "big"), 1, stderr=b"kv.sh: integer overflow: big\n")
        self.assertFalse((self.store / ".lock").exists())
        start = time.monotonic()
        self.assert_ok(self.kv("set", "b", "2", env={"KV_LOCK_TIMEOUT": "5"}))
        self.assertLess(time.monotonic() - start, 3)
        self.assert_store({b"a": b"text", b"b": b"2", b"big": b"999999999999999999"})

    def test_concurrent_incr(self):
        with ThreadPoolExecutor(max_workers=20) as pool:
            results = list(pool.map(lambda _: self.kv("incr", "counter", env=SLOW_LOCK, timeout=90), range(20)))
        for result in results:
            self.assertEqual((result.returncode, result.stderr), (0, b""))
        self.assertEqual(sorted(int(r.stdout) for r in results), list(range(1, 21)))
        self.assert_store({b"counter": b"20"})

    def test_concurrent_incr_with_stale_lock(self):
        self.make_lock(self.dead_pid())
        with ThreadPoolExecutor(max_workers=15) as pool:
            results = list(pool.map(lambda _: self.kv("incr", "counter", "2", env=SLOW_LOCK, timeout=90), range(15)))
        for result in results:
            self.assertEqual((result.returncode, result.stderr), (0, b""))
        self.assertEqual(sorted(int(r.stdout) for r in results), list(range(2, 31, 2)))
        self.assert_store({b"counter": b"30"})

    def test_concurrent_writers_on_distinct_keys(self):
        keys = [b"key %d/x" % i for i in range(15)]
        with ThreadPoolExecutor(max_workers=15) as pool:
            results = list(pool.map(lambda k: self.kv("set", k, k + b"\n", env=SLOW_LOCK, timeout=90), keys))
        for result in results:
            self.assert_ok(result)
        self.assert_store({k: k + b"\n" for k in keys})


class RandomDifferentialTest(Base):
    KEYS = [b"a", b"a b", b"-n", b"..", b"x/y", b"*", b"[a]", b"\\", b"caf\xc3\xa9", b"%2F", b"cnt", b"n-1",
            b"", b"bad\nkey", b".dot"]
    VALUES = NASTY_VALUES[:30] + [b"0", b"-7", b"007", b"12", b"x"]
    STEPS = [b"1", b"-1", b"10", b"007", b"-0", b"x", b"+1", b"999999999999999999"]

    def random_args(self, rng: random.Random) -> list[bytes]:
        op = rng.choice(["set", "set", "get", "get", "del", "list", "export", "incr", "incr", "bad"])
        key = rng.choice(self.KEYS)
        if op == "set":
            return [b"set", key, rng.choice(self.VALUES)]
        if op in ("get", "del"):
            return [op.encode(), key]
        if op == "list":
            return [b"list"] + ([rng.choice([b"a", b"", b"*", b"\\", b"c", b".", b"-"])] if rng.random() < 0.6 else [])
        if op == "export":
            return [b"export"] + ([b"--json"] if rng.random() < 0.5 else [])
        if op == "incr":
            return [b"incr", key] + ([rng.choice(self.STEPS)] if rng.random() < 0.6 else [])
        return rng.choice([[b"set", key], [b"get"], [b"bogus"], [b"export", b"-j"], [b"incr", key, b"1", b"2"]])

    def test_random_sequences(self):
        for seed in range(8):
            rng = random.Random(seed)
            model = Model()
            store = self.root / f"seed {seed}"
            for step in range(30):
                args = self.random_args(rng)
                with self.subTest(seed=seed, step=step, args=args):
                    expected = model.run(args)
                    result = self.kv("-d", bytes(store), *args, store=False)
                    self.assert_result(result, *expected)
            self.assert_store(model.data, store)


if __name__ == "__main__":
    unittest.main()
