import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "wordfreq.py"

# 15 distinct words with distinct counts 15..1, so the top-10 cut is unambiguous.
MANY = " ".join(f"w{chr(97 + i)} " * (15 - i) for i in range(15))
MIXED = "The the THE cat Cat dog. It's it's IT'S a A an"


class Result:
    def __init__(self, completed):
        self.code = completed.returncode
        self.out = completed.stdout
        self.err = completed.stderr


def wordfreq(text, *args):
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "input.txt"
        path.write_text(text, encoding="utf-8")
        return Result(subprocess.run([sys.executable, str(SCRIPT), str(path), *args],
                                     capture_output=True, text=True, timeout=30))


def lines(*pairs):
    return "".join(f"{count}\t{word}\n" for count, word in pairs)


class HiddenWordfreqTest(unittest.TestCase):
    def test_visible_behaviour_kept(self):
        self.assertEqual(wordfreq("b a b").out, "2\tb\n1\ta\n")
        self.assertEqual(wordfreq("pear apple fig").out, "1\tapple\n1\tfig\n1\tpear\n")

    def test_default_top_is_ten(self):
        result = wordfreq(MANY)
        self.assertEqual(result.code, 0)
        self.assertEqual(result.out, lines(*[(15 - i, f"w{chr(97 + i)}") for i in range(10)]))

    def test_top_n(self):
        self.assertEqual(wordfreq(MANY, "--top", "3").out, lines((15, "wa"), (14, "wb"), (13, "wc")))
        self.assertEqual(wordfreq(MANY, "--top=1").out, lines((15, "wa")))

    def test_top_larger_than_vocabulary_prints_all(self):
        self.assertEqual(wordfreq("x y x", "--top", "50").out, lines((2, "x"), (1, "y")))

    def test_top_cut_respects_tie_order(self):
        self.assertEqual(wordfreq("d c b a a", "--top", "2").out, lines((2, "a"), (1, "b")))

    def test_invalid_top_is_usage_error(self):
        for value in ["0", "-1", "abc", "2.5"]:
            with self.subTest(value=value):
                result = wordfreq("a b", "--top", value)
                self.assertEqual(result.code, 2)
                self.assertEqual(result.out, "")
                self.assertTrue(result.err.strip())

    def test_case_sensitive_by_default(self):
        out = wordfreq(MIXED, "--top", "20").out
        self.assertIn("1\tThe\n", out)
        self.assertIn("1\tTHE\n", out)
        self.assertIn("1\tthe\n", out)

    def test_ignore_case(self):
        result = wordfreq(MIXED, "--ignore-case", "--top", "20")
        self.assertEqual(result.code, 0)
        self.assertEqual(result.out, lines((3, "it's"), (3, "the"), (2, "a"), (2, "cat"), (1, "an"), (1, "dog")))

    def test_min_length(self):
        text = "a bb ccc dddd bb ccc ccc a"
        self.assertEqual(wordfreq(text, "--min-length", "3").out, lines((3, "ccc"), (1, "dddd")))
        self.assertEqual(wordfreq(text, "--min-length", "1").out,
                         lines((3, "ccc"), (2, "a"), (2, "bb"), (1, "dddd")))

    def test_invalid_min_length_is_usage_error(self):
        for value in ["0", "-2", "x"]:
            with self.subTest(value=value):
                self.assertEqual(wordfreq("a", "--min-length", value).code, 2)

    def test_options_combine(self):
        result = wordfreq(MIXED, "--ignore-case", "--min-length", "3", "--top", "2")
        self.assertEqual(result.out, lines((3, "it's"), (3, "the")))

    def test_unreadable_file_still_exits_one(self):
        result = Result(subprocess.run([sys.executable, str(SCRIPT), "/nonexistent/x.txt"],
                                       capture_output=True, text=True, timeout=30))
        self.assertEqual(result.code, 1)
        self.assertTrue(result.err.startswith("wordfreq: cannot read /nonexistent/x.txt: "))


if __name__ == "__main__":
    unittest.main()
