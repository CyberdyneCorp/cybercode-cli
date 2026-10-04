"""Hidden contract tests for termtable (README.md, sections 1-8)."""

import random
import unittest

import grader_ref_termtable as ref
from termtable import (align, allocate, char_width, display_width, graphemes, normalize,
                       render_table, truncate, wrap)

CHAR_WIDTHS = [
    # width 1: Na, N, A (ambiguous) and H (halfwidth)
    ("a", 1), ("Z", 1), (" ", 1), ("~", 1), ("-", 1), ("\u00e9", 1), ("\u00df", 1), ("\u00b1", 1),
    ("\u03a9", 1), ("\u03b1", 1), ("\u0416", 1), ("\u2500", 1), ("\u2026", 1), ("\uff71", 1),
    ("\u00a5", 1), ("\u00a0", 1), ("\u0e01", 1),
    # width 2: W and F
    ("\u4e2d", 2), ("\u6587", 2), ("\ud55c", 2), ("\u3072", 2), ("\u30ab", 2), ("\u3001", 2),
    ("\uac00", 2), ("\uff21", 2), ("\uff10", 2), ("\uff01", 2), ("\u3000", 2), ("\uffe5", 2),
    # width 0: listed characters, Mn, Me, Cf (checked before East Asian Width)
    ("\u0301", 0), ("\u0300", 0), ("\u0308", 0), ("\u0327", 0), ("\u20dd", 0), ("\u20de", 0),
    ("\u200b", 0), ("\u200d", 0), ("\ufeff", 0), ("\u2060", 0), ("\u00ad", 0), ("\u200e", 0),
    ("\u0e34", 0), ("\u034f", 0), ("\u0901", 0), ("\u3099", 0),
]

DISPLAY_WIDTHS = [
    ("", 0), ("hello", 5), ("\u4e2d\u6587\u5b57", 6), ("\uff21\uff22\uff23", 6), ("abc\u4e2d", 5),
    ("e\u0301", 1), ("Cafe\u0301", 4), ("x\u0301\u0323", 1), ("\u1100\u1161", 2), ("\u212b", 1),
    ("a\u200bb", 2), ("\ufeffabc", 3), ("a\u20dd", 1), ("\u30a2\u3099", 2), ("\u0e01\u0e34\u0e19", 2),
    ("\uff71\uff72\uff73", 3), ("\u00ad", 0), ("ab\tc", 5), ("abcd\tc", 9), ("\u4e2d\tc", 5),
    ("e\u0301\tc", 5), ("\t", 4), ("a\t\tb", 9), ("\u4e2d\u6587\u5b57\tx", 9), ("\uff21\tb", 5),
    ("x\u20dd\u200b\t|", 5),
]

NORMALIZED = [
    ("e\u0301", "\u00e9"), ("ab\tc", "ab  c"), ("abcd\tc", "abcd    c"), ("\u4e2d\tc", "\u4e2d  c"),
    ("e\u0301\tc", "\u00e9   c"), ("x\u0301\tc", "x\u0301   c"), ("\u1100\u1161\tz", "\uac00  z"),
    ("\uf900", "\u8c48"), ("\u200b\tx", "\u200b    x"), ("\u4e2d\u6587\u5b57\tx", "\u4e2d\u6587\u5b57  x"),
    ("\uff21\uff22\t.", "\uff21\uff22    ."), ("a\u0308\u0301\t", "\u00e4\u0301   "),
    ("x\u0301\u0323", "x\u0323\u0301"), ("plain", "plain"),
]

GRAPHEMES = [
    ("abc", ["a", "b", "c"]), ("e\u0301x", ["\u00e9", "x"]), ("x\u0301\u0323y", ["x\u0323\u0301", "y"]),
    ("\u200bab", ["\u200b", "a", "b"]), ("\u4e2d\u200d\u6587", ["\u4e2d\u200d", "\u6587"]),
    ("a\tb", ["a", " ", " ", " ", "b"]), ("", []), ("\u0301\u0302a", ["\u0301\u0302", "a"]),
    ("\u30a2\u3099\u30a4", ["\u30a2\u3099", "\u30a4"]), ("a\u20dd\ufeffb", ["a\u20dd\ufeff", "b"]),
]

TRUNCATIONS = [
    ("hello world", 8, "hello w\u2026"), ("hello", 5, "hello"), ("hello", 4, "hel\u2026"),
    ("anything", 1, "\u2026"), ("a", 1, "a"), ("ab", 1, "\u2026"), ("ab cd", 4, "ab \u2026"),
    ("\u4e2d\u6587\u5b57\u5e55", 6, "\u4e2d\u6587 \u2026"), ("\u4e2d\u6587\u5b57\u5e55", 7, "\u4e2d\u6587\u5b57\u2026"),
    ("\u4e2d\u6587\u5b57\u5e55", 8, "\u4e2d\u6587\u5b57\u5e55"), ("\u4e2d\u6587\u5b57\u5e55", 2, " \u2026"),
    ("\u4e2d\u6587\u5b57\u5e55", 3, "\u4e2d\u2026"), ("\u4e2da\u6587", 3, "\u4e2d\u2026"),
    ("a\u4e2d\u6587", 3, "a \u2026"), ("Cafe\u0301", 4, "Caf\u00e9"), ("Cafe\u0301s", 4, "Caf\u2026"),
    ("x\u0301q\u0301j\u0301", 3, "x\u0301q\u0301j\u0301"), ("x\u0301q\u0301j\u0301w", 3, "x\u0301q\u0301\u2026"),
    ("x\u0301yz", 2, "x\u0301\u2026"), ("\uff21\uff22\uff23", 5, "\uff21\uff22\u2026"),
    ("\uff21\uff22\uff23", 4, "\uff21 \u2026"), ("\ud55c\uad6d\uc5b4", 5, "\ud55c\uad6d\u2026"),
    ("ab\tcd", 5, "ab  \u2026"), ("ab\tc", 5, "ab  c"), ("a\u200bbcdef", 3, "a\u200bb\u2026"),
    ("abc\ufeff", 3, "abc\ufeff"), ("abcd\u20dd", 3, "ab\u2026"), ("abc\u20dd", 3, "abc\u20dd"),
    ("\u30a2\u3099\u30a4\u30a6", 4, "\u30a2\u3099 \u2026"), ("e\u0301e\u0301e\u0301", 2, "\u00e9\u2026"),
    ("\uff71\uff72\uff73\uff74\uff75", 4, "\uff71\uff72\uff73\u2026"), ("\u0e01\u0e34\u0e19\u0e01", 2, "\u0e01\u0e34\u2026"),
]

ALIGNMENTS = [
    ("ab", 5, "left", "ab   "), ("ab", 5, "right", "   ab"), ("ab", 5, "center", " ab  "),
    ("abc", 6, "center", " abc  "), ("abc", 5, "center", " abc "), ("ab", 4, "center", " ab "),
    ("a", 4, "center", " a  "), ("\u4e2d\u6587", 6, "left", "\u4e2d\u6587  "),
    ("\u4e2d\u6587", 6, "right", "  \u4e2d\u6587"), ("\u4e2d\u6587", 5, "center", "\u4e2d\u6587 "),
    ("\u4e2d", 5, "center", " \u4e2d  "), ("e\u0301", 3, "right", "  \u00e9"),
    ("x\u0301", 3, "center", " x\u0301 "), ("hello world", 8, "right", "hello w\u2026"),
    ("\u4e2d\u6587\u5b57\u5e55", 6, "center", "\u4e2d\u6587 \u2026"),
    ("\u4e2d\u6587\u5b57\u5e55", 7, "left", "\u4e2d\u6587\u5b57\u2026"), ("\u212b", 2, "left", "\u00c5 "),
    ("ab\tc", 7, "right", "  ab  c"), ("", 3, "center", "   "), ("\uff21\uff22", 5, "center", "\uff21\uff22 "),
    ("\uff21", 5, "right", "   \uff21"), ("a\u20dd\u200b", 4, "center", " a\u20dd\u200b  "),
]

WRAPS = [
    ("the quick brown fox", 10, ["the quick", "brown fox"]), ("abcdefgh ij", 3, ["abc", "def", "gh", "ij"]),
    ("well-known \u4e2d\u6587\u5b57", 5, ["well-", "known", "\u4e2d\u6587", "\u5b57"]), ("", 5, []),
    ("   ", 5, []), ("  a   b  ", 5, ["a b"]), ("a b c d", 3, ["a b", "c d"]), ("aaa bbb", 3, ["aaa", "bbb"]),
    ("aaaa b", 3, ["aaa", "a b"]), ("ab cdefg", 4, ["ab", "cdef", "g"]), ("ab cdefg h", 4, ["ab", "cdef", "g h"]),
    ("\u4e2d\u6587 \u5b57", 5, ["\u4e2d\u6587", "\u5b57"]), ("\u4e2d\u6587 \u5b57", 7, ["\u4e2d\u6587 \u5b57"]),
    ("\u4e2d\u6587\u5b57", 5, ["\u4e2d\u6587", "\u5b57"]), ("\u4e2d\u6587\u5b57", 3, ["\u4e2d", "\u6587", "\u5b57"]),
    ("a\u4e2d\u6587", 4, ["a\u4e2d", "\u6587"]), ("x\u0301q\u0301j\u0301", 2, ["x\u0301q\u0301", "j\u0301"]),
    ("e\u0301e\u0301e\u0301 ab", 4, ["\u00e9\u00e9\u00e9", "ab"]), ("foo\u200bbar baz", 6, ["foo\u200bbar", "baz"]),
    ("a\u3000b c", 4, ["a\u3000b", "c"]), ("a\u00a0b c", 3, ["a\u00a0b", "c"]), ("one\ttwo", 8, ["one two"]),
    ("one\ttwo", 5, ["one", "two"]), ("a\t\tb", 4, ["a b"]), ("hello world", 2, ["he", "ll", "o", "wo", "rl", "d"]),
    ("\uff21\uff22\uff23 \uff24\uff25", 4, ["\uff21\uff22", "\uff23", "\uff24\uff25"]), ("a b", 2, ["a", "b"]),
    ("ab", 2, ["ab"]), ("re-enter the co-op", 8, ["re-enter", "the", "co-op"]),
    ("Cafe\u0301 au lait", 7, ["Caf\u00e9 au", "lait"]), ("\u30a2\u3099\u30a2\u3099\u30a2\u3099", 4, ["\u30a2\u3099\u30a2\u3099", "\u30a2\u3099"]),
]

ALLOCATIONS = [
    ([10, 4, 10], 20, [5, 4, 5]), ([5, 5], None, [5, 5]), ([5, 5], 13, [5, 5]), ([5, 5], 12, [5, 4]),
    ([5, 5], 11, [4, 4]), ([1, 8, 2], 11, [1, 2, 2]), ([3, 3, 3], 13, [3, 2, 2]), ([6, 9, 7], 20, [5, 5, 4]),
    ([4], 2, [2]), ([4, 4, 4], 14, [3, 3, 2]), ([7, 3, 7, 3], 25, [5, 3, 5, 3]), ([12, 11], 15, [6, 6]),
]

ALLOCATION_ERRORS = [([1, 8, 2], 10), ([2, 2], 5), ([4], 1), ([3, 9, 3], 10)]

TABLES = [
    (dict(rows=[["\uff21\uff22", "x"]], headers=["k", "v"]), "k    | v\n-----+--\n\uff21\uff22 | x"),
    (dict(rows=[["Cafe\u0301", "10"], ["Zu\u0308rich", "7"]], headers=["city", "n"], align=["left", "right"]),
     "city   |  n\n-------+---\nCaf\u00e9   | 10\nZ\u00fcrich |  7"),
    (dict(rows=[["a", "\u4e2d"]], headers=["xyz", "w"], align=["center", "center"]),
     "xyz | w \n----+---\n a  | \u4e2d"),
    (dict(rows=[["\u4e2d\u6587\u5b57\u5e55\u6d4b\u8bd5", "hello world"]], headers=["title", "greeting"], max_width=15),
     "title  | greet\u2026\n-------+-------\n\u4e2d\u6587 \u2026 | hello\u2026"),
    (dict(rows=[["\u4e2d\u6587\u5b57\u5e55\u6d4b\u8bd5", "hello world"], ["ok", "a b"]], headers=["title", "greeting"],
          max_width=15, overflow="wrap", align=["left", "center"]),
     "title  | greeti\n       |   ng  \n-------+-------\n\u4e2d\u6587\u5b57 | hello \n\u5e55\u6d4b\u8bd5 | world \nok     |  a b  "),
    (dict(rows=[["a\tb", "x"], ["\u4e2d\tc", "yy"]]), "a   b | x \n\u4e2d  c | yy"),
    (dict(rows=[["x\u20dd\u200b", "\ufeffab"], ["\uff71\uff72\uff73", "\u03a9"]], headers=["c1", "c2"], align=["right", "center"]),
     " c1 | c2\n----+---\n  x\u20dd\u200b | \ufeffab\n\uff71\uff72\uff73 | \u03a9 "),
    (dict(rows=[["the quick brown fox", "1"], ["", "22"]], headers=["text", "n"], max_width=14, overflow="wrap",
          align=["left", "right"]),
     "text      |  n\n----------+---\nthe quick |  1\nbrown fox |   \n          | 22"),
    (dict(rows=[], headers=["a", "bb"]), "a | bb\n--+---"),
    (dict(rows=[["", ""]]), "  |  "),
    (dict(rows=[["aaaa", "bbbb", "cccc"]], headers=["x", "y", "z"], max_width=14),
     "x   | y   | z \n----+-----+---\naa\u2026 | bb\u2026 | c\u2026"),
    (dict(rows=[["aaaaaa", "bb", "cccccc"]], max_width=14), "aa\u2026 | bb | cc\u2026"),
    (dict(rows=[]), ""),
    (dict(rows=[("\u4e2d\u6587", "1")], headers=("name", "n")), "name | n\n-----+--\n\u4e2d\u6587 | 1"),
    (dict(rows=[[" a  b", "x"], ["cc", "y"]], headers=["k", "v"], overflow="wrap"), "k     | v\n------+--\n a  b | x\ncc    | y"),
    (dict(rows=[["\uff21\uff22\uff23 \uff24", "z"]], max_width=8, overflow="wrap"),
     "\uff21\uff22 | z\n\uff23   |  \n\uff24   |  "),
]

TABLE_ERRORS = [
    dict(rows=[["a", "b"], ["c"]]), dict(rows=[["a"]], headers=["x", "y"]), dict(rows=[[]]),
    dict(rows=[["a", "b"]], align=["left"]), dict(rows=[["a"]], align=["middle"]),
    dict(rows=[["a"]], overflow="clip"), dict(rows=[["aaaa", "bbbb"]], max_width=6),
]


class WidthTest(unittest.TestCase):
    def test_char_width(self):
        for ch, expected in CHAR_WIDTHS:
            with self.subTest(char=f"U+{ord(ch):04X}"):
                self.assertEqual(char_width(ch), expected)

    def test_display_width(self):
        for text, expected in DISPLAY_WIDTHS:
            with self.subTest(text=ascii(text)):
                self.assertEqual(display_width(text), expected)

    def test_normalize(self):
        for text, expected in NORMALIZED:
            with self.subTest(text=ascii(text)):
                self.assertEqual(normalize(text), expected)

    def test_graphemes(self):
        for text, expected in GRAPHEMES:
            with self.subTest(text=ascii(text)):
                self.assertEqual(graphemes(text), expected)


class TextTest(unittest.TestCase):
    def test_truncate(self):
        for text, width, expected in TRUNCATIONS:
            with self.subTest(text=ascii(text), width=width):
                self.assertEqual(truncate(text, width), expected)

    def test_align(self):
        for text, width, how, expected in ALIGNMENTS:
            with self.subTest(text=ascii(text), width=width, how=how):
                self.assertEqual(align(text, width, how), expected)

    def test_align_default_is_left(self):
        self.assertEqual(align("\u4e2d", 4), "\u4e2d  ")

    def test_wrap(self):
        for text, width, expected in WRAPS:
            with self.subTest(text=ascii(text), width=width):
                self.assertEqual(wrap(text, width), expected)

    def test_invalid_widths(self):
        for call in (lambda: truncate("abc", 0), lambda: align("abc", 0), lambda: align("abc", 3, "middle"),
                     lambda: wrap("abc", 1), lambda: wrap("abc", 0)):
            with self.assertRaises(ValueError):
                call()


class AllocateTest(unittest.TestCase):
    def test_allocate(self):
        for natural, max_width, expected in ALLOCATIONS:
            with self.subTest(natural=natural, max_width=max_width):
                given = list(natural)
                self.assertEqual(allocate(given, max_width), expected)
                self.assertEqual(given, natural, "allocate must not modify its argument")

    def test_allocate_errors(self):
        for natural, max_width in ALLOCATION_ERRORS:
            with self.subTest(natural=natural, max_width=max_width), self.assertRaises(ValueError):
                allocate(natural, max_width)


class TableTest(unittest.TestCase):
    def test_tables(self):
        for index, (kwargs, expected) in enumerate(TABLES):
            kwargs = dict(kwargs)
            rows, headers = kwargs.pop("rows"), kwargs.pop("headers", None)
            with self.subTest(case=index):
                self.assertEqual(render_table(rows, headers, **kwargs), expected)

    def test_table_errors(self):
        for index, kwargs in enumerate(TABLE_ERRORS):
            kwargs = dict(kwargs)
            rows, headers = kwargs.pop("rows"), kwargs.pop("headers", None)
            with self.subTest(case=index), self.assertRaises(ValueError):
                render_table(rows, headers, **kwargs)


BASES = list("abcxyzAZ09-.,") + ["e", "u", "\u4e2d", "\u6587", "\ud55c", "\u3072", "\u30ab", "\uff21", "\uff10",
                                  "\uff71", "\u00e9", "\u03a9", "\u2500", "\u3000", "\u00a0", "\u0e01"]
MARKS = ["\u0301", "\u0308", "\u0323", "\u20dd", "\u200b", "\u200d", "\ufeff", "\u00ad", "\u0e34", "\u3099"]


def random_text(rng: random.Random, max_tokens: int = 14) -> str:
    """Random text whose zero-width characters always follow a non-space base character."""
    parts = []
    for _ in range(rng.randint(0, max_tokens)):
        roll = rng.random()
        if roll < 0.18:
            parts.append(" " * rng.randint(1, 3))
        elif roll < 0.23:
            parts.append("\t")
        else:
            parts.append(rng.choice(BASES) + "".join(rng.choice(MARKS) for _ in range(rng.choice((0, 0, 0, 1, 2)))))
    return "".join(parts)


def outcome(func, *args, **kwargs):
    try:
        return ("ok", func(*args, **kwargs))
    except ValueError:
        return ("ValueError",)


class DifferentialTest(unittest.TestCase):
    """Random inputs compared with a reference implementation of the README contract.

    Each test stops at the first mismatch and reports that input.
    """

    def assert_same(self, label, actual, expected):
        if actual != expected:
            self.fail(f"{label}: got {actual!r}, expected {expected!r}")

    def test_text_functions(self):
        rng = random.Random(20261004)
        for _ in range(400):
            text = random_text(rng)
            self.assert_same(f"normalize({text!r})", normalize(text), ref.normalize(text))
            self.assert_same(f"display_width({text!r})", display_width(text), ref.display_width(text))
            self.assert_same(f"graphemes({text!r})", graphemes(text), ref.graphemes(text))
            for width in (1, 2, 3, 5, 8, 13):
                self.assert_same(f"truncate({text!r}, {width})", truncate(text, width), ref.truncate(text, width))
                for how in ("left", "right", "center"):
                    self.assert_same(f"align({text!r}, {width}, {how!r})", align(text, width, how),
                                     ref.align(text, width, how))
                if width >= 2:
                    self.assert_same(f"wrap({text!r}, {width})", wrap(text, width), ref.wrap(text, width))

    def test_allocate(self):
        rng = random.Random(7)
        for _ in range(300):
            natural = [rng.randint(1, 15) for _ in range(rng.randint(1, 5))]
            max_width = rng.randint(1, 60)
            self.assert_same(f"allocate({natural}, {max_width})", outcome(allocate, natural, max_width),
                             outcome(ref.allocate, natural, max_width))

    def test_tables(self):
        rng = random.Random(42)
        for _ in range(150):
            columns = rng.randint(1, 4)
            rows = [[random_text(rng, 8) for _ in range(columns)] for _ in range(rng.randint(0, 4))]
            headers = [random_text(rng, 3) for _ in range(columns)] if rng.random() < 0.7 else None
            kwargs = {
                "max_width": rng.choice((None, rng.randint(columns * 5, 50))),
                "align": rng.choice((None, [rng.choice(("left", "right", "center")) for _ in range(columns)])),
                "overflow": rng.choice(("truncate", "wrap")),
            }
            self.assert_same(f"render_table({rows!r}, {headers!r}, **{kwargs!r})",
                             outcome(render_table, rows, headers, **kwargs),
                             outcome(ref.render_table, rows, headers, **kwargs))


if __name__ == "__main__":
    unittest.main()
