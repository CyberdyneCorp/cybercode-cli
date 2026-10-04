import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "wordfreq.py"


def wordfreq(text: str, *args: str) -> subprocess.CompletedProcess:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "input.txt"
        path.write_text(text, encoding="utf-8")
        return subprocess.run([sys.executable, str(SCRIPT), str(path), *args],
                              capture_output=True, text=True, timeout=30)


class WordfreqTest(unittest.TestCase):
    def test_counts_words(self):
        result = wordfreq("b a b")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "2\tb\n1\ta\n")

    def test_ties_sorted_by_word(self):
        self.assertEqual(wordfreq("pear apple fig").stdout, "1\tapple\n1\tfig\n1\tpear\n")


if __name__ == "__main__":
    unittest.main()
