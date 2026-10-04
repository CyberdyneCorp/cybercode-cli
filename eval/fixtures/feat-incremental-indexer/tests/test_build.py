import json
import os
import tempfile
import unittest
from pathlib import Path

from indexer import IndexConfig, build_index, load_index, search_file


def write(root: Path, rel: str, text: str, mtime: int) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    os.utime(path, ns=(mtime * 10**9, mtime * 10**9))


class BuildTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name) / "docs"
        self.index_path = Path(self.tmp.name) / "index.json"
        write(self.root, "a.txt", "The quick brown fox", 1_700_000_000)
        write(self.root, "notes/b.md", "A quick summary of release notes", 1_700_000_001)
        write(self.root, "build/c.txt", "quick build output", 1_700_000_002)
        write(self.root, "script.py", "quick = 1", 1_700_000_003)

    def tearDown(self):
        self.tmp.cleanup()

    def test_full_build_and_search(self):
        build_index(self.root, self.index_path, IndexConfig(ignore=("build",)))
        index, meta = load_index(self.index_path)
        self.assertEqual([d.path for d in index.docs], ["a.txt", "notes/b.md"])
        self.assertEqual(meta.ignore, ("build",))
        self.assertEqual(search_file(self.index_path, "quick"), ["a.txt", "notes/b.md"])
        self.assertEqual(search_file(self.index_path, '"release notes"'), ["notes/b.md"])
        self.assertEqual(search_file(self.index_path, "fox OR summary"), ["a.txt", "notes/b.md"])

    def test_index_file_is_canonical_json(self):
        build_index(self.root, self.index_path)
        raw = self.index_path.read_bytes()
        data = json.loads(raw)
        self.assertEqual(data["version"], 2)
        self.assertEqual(data["docs"][0]["mtime_ns"], 1_700_000_000 * 10**9)
        expected = json.dumps(data, sort_keys=True, ensure_ascii=False, separators=(",", ":")) + "\n"
        self.assertEqual(raw, expected.encode("utf-8"))


if __name__ == "__main__":
    unittest.main()
