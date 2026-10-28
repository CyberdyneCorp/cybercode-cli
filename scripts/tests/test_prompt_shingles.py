"""The prompt provenance fixture: normalization and the clean-room check."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import prompt_shingles as shingles


class Normalization(unittest.TestCase):
    def test_vector_shared_with_the_rust_test(self):
        run = shingles.words(
            "One, two: three\n- four *five* six seven-eight nine ten eleven twelve."
        )
        self.assertEqual(len(run), shingles.WIDTH)
        # Pinned in crates/cyber-core/tests/prompt_provenance.rs as well.
        self.assertEqual(shingles.shingle_hash(run), "aba257081516")
        self.assertEqual(shingles.words("don’t café"), ["don", "t", "caf"])

    def test_short_text_has_no_shingles(self):
        self.assertEqual(shingles.shingles("only eleven words " * 3), set())

    def test_reference_assets_cover_the_source_tree_and_asset_folders(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            names = [
                "codex-rs/core/prompt.md",
                "codex-rs/core/assets/tools/patch.lark",
                "codex-rs/tui/tips.txt",
                "codex-rs/core/assets/agent/roles.toml",
                "codex-rs/core/src/lib.rs",
                "codex-rs/target/debug/notes.md",
                "codex-rs/node_modules/pkg/README.md",
                "docs/config.md",
                "sdk/templates/greeting.yaml",
                "sdk/assets/logo.png",
            ]
            for name in names:
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text("text")
            found = [p.relative_to(root).as_posix() for p in shingles.reference_files(root)]
            self.assertEqual(
                found,
                [
                    "codex-rs/core/assets/agent/roles.toml",
                    "codex-rs/core/assets/tools/patch.lark",
                    "codex-rs/core/prompt.md",
                    "codex-rs/tui/tips.txt",
                    "sdk/templates/greeting.yaml",
                ],
            )

    def test_a_copied_run_is_reported(self):
        reference = "the quick brown fox jumps over the lazy dog while the cat sleeps soundly"
        with tempfile.TemporaryDirectory() as tmp:
            ours = Path(tmp) / "ours.md"
            ours.write_text("Intro. The quick brown fox jumps over the lazy dog, while the cat sleeps!")
            found = shingles.matches(shingles.shingles(reference), [ours])
            # Case and punctuation do not hide a copy.
            self.assertEqual({name for name, _ in found}, {"ours.md"})
            self.assertIn("the quick brown fox jumps over the lazy dog while the cat", [w for _, w in found])
            ours.write_text("The quick brown fox jumps over a lazy dog while the cat sleeps soundly.")
            self.assertEqual(shingles.matches(shingles.shingles(reference), [ours]), [])


class Provenance(unittest.TestCase):
    def test_our_prompts_share_no_twelve_word_run(self):
        self.assertGreater(len(shingles.fixture_hashes()), 50_000)
        self.assertEqual(shingles.matches(), [])

    def test_every_base_prompt_is_checked(self):
        names = [path.name for path in shingles.our_assets()]
        self.assertTrue({"core.md", "openai.md", "anthropic.md", "gemini.md", "local.md"} <= set(names))


if __name__ == "__main__":
    unittest.main()
