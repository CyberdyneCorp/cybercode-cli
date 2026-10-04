import json
import os
import tempfile
import unittest
from pathlib import Path

from indexer import IndexConfig, IndexStats, build_index

BASE = 1_700_000_000 * 10**9


class IncrementalBuildTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name) / "docs"
        self.index_path = Path(self.tmp.name) / "index.json"
        self.full_path = Path(self.tmp.name) / "full.json"
        self.clock = 0
        self.write("a.txt", "the quick brown fox")
        self.write("b/c.md", "release notes")
        self.write("b/d.txt", "unique zebra")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, rel, text, mtime=None):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        self.touch(rel, mtime)

    def touch(self, rel, mtime=None):
        if mtime is None:
            self.clock += 1
            mtime = BASE + self.clock * 1_000_000_007
        os.utime(self.root / rel, ns=(mtime, mtime))

    def build(self, config=None):
        stats = build_index(self.root, self.index_path, config, incremental=True)
        build_index(self.root, self.full_path, config)
        self.assertEqual(self.index_path.read_bytes(), self.full_path.read_bytes())
        return stats

    def test_first_build_is_full(self):
        self.assertEqual(self.build(), IndexStats(3, 0, 0, True))

    def test_unchanged_and_touched_files_are_reused(self):
        self.build()
        self.assertEqual(self.build(), IndexStats(0, 3, 0, False))
        self.touch("a.txt")
        self.assertEqual(self.build(), IndexStats(0, 3, 0, False))

    def test_modified_added_and_deleted_files(self):
        self.build()
        self.write("a.txt", "the slow brown fox")
        self.write("0.txt", "first file shifts every id")
        (self.root / "b/d.txt").unlink()
        self.assertEqual(self.build(), IndexStats(2, 1, 1, False))
        self.assertNotIn("zebra", json.loads(self.index_path.read_bytes())["postings"])

    def test_size_change_with_same_mtime(self):
        self.build()
        mtime = (self.root / "a.txt").stat().st_mtime_ns
        self.write("a.txt", "the quick brown fox jumps", mtime=mtime)
        self.assertEqual(self.build(), IndexStats(1, 2, 0, False))

    def test_rename_is_remove_plus_add(self):
        self.build()
        os.rename(self.root / "b/d.txt", self.root / "e.txt")
        self.assertEqual(self.build(), IndexStats(1, 2, 1, False))

    def test_settings_changes_force_full_build(self):
        self.build()
        self.assertEqual(self.build(IndexConfig(stemming=False)), IndexStats(3, 0, 0, True))
        (self.root / ".indexignore").write_text("b\n", encoding="utf-8")
        self.assertEqual(self.build(IndexConfig(stemming=False)), IndexStats(1, 0, 0, True))

    def test_unreadable_previous_index_forces_full_build(self):
        self.build()
        self.index_path.write_text("{}", encoding="utf-8")
        self.assertEqual(self.build(), IndexStats(3, 0, 0, True))


if __name__ == "__main__":
    unittest.main()
