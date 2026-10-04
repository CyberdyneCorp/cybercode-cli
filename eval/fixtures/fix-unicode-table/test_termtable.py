import unittest

from termtable import display_width, render_table, truncate


class TruncateTest(unittest.TestCase):
    def test_truncate_ascii(self):
        self.assertEqual(truncate("hello world", 8), "hello w…")
        self.assertEqual(truncate("short", 8), "short")


class TableTest(unittest.TestCase):
    def test_ascii_table(self):
        table = render_table([["apple", "3"], ["kiwi", "12"]], headers=["fruit", "qty"],
                             align=["left", "right"])
        self.assertEqual(table, "fruit | qty\n------+----\napple |   3\nkiwi  |  12")

    def test_cjk_table(self):
        self.assertEqual(display_width("中文"), 4)
        table = render_table([["中文", "1"], ["abc", "22"]], headers=["name", "n"])
        self.assertEqual(table, "name | n \n-----+---\n中文 | 1 \nabc  | 22")


if __name__ == "__main__":
    unittest.main()
