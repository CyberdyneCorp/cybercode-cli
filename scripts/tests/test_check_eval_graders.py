"""Cross-language fixture identity and non-following source aliases."""

import hashlib
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import check_eval_graders as graders


@unittest.skipUnless(os.name == "posix", "source alias controls require symlinks")
class SourceIdentity(unittest.TestCase):
    def test_alias_is_hashed_as_literal_target_without_duplicate_contents(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "pkg").mkdir()
            (root / "pkg/file").write_bytes(b"abc")
            (root / "alias").symlink_to("pkg", target_is_directory=True)
            expected = hashlib.sha256(b"alias\0link\0pkgpkg/file\x003\0abc")
            self.assertEqual(
                graders.tree_sha256(root), "sha256:" + expected.hexdigest()
            )

    def test_private_and_escaping_aliases_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            alias = root / "alias"
            for target in ["../escape", "/absolute", ".GiT/config", "x\\y", "C:escape"]:
                with self.subTest(target=target):
                    alias.symlink_to(target)
                    with self.assertRaisesRegex(ValueError, "unsafe source alias"):
                        graders.tree_sha256(root)
                    alias.unlink()

    def test_snapshot_records_link_identity_without_reading_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "alias").symlink_to("missing")
            before = graders.snapshot(root)
            (root / "alias").unlink()
            (root / "alias").symlink_to("another-missing")
            self.assertNotEqual(before, graders.snapshot(root))


if __name__ == "__main__":
    unittest.main()
