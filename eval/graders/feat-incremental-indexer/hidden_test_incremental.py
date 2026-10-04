"""Hidden tests for feat-incremental-indexer (see the fixture's docs/INCREMENTAL.md).

Every build of the agent's `indexer` is compared with a full build by the frozen reference
implementation (`_grader_ref/refindexer`) of the same tree and config: the index bytes must
be identical and the stats must equal the counts implied by the spec.
"""

import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "_grader_ref"))

import indexer  # noqa: E402  (the agent's package)
import refindexer  # noqa: E402  (frozen reference)

BASE_SECONDS = 1_700_000_000
VOCAB = (
    "the a of and to in is it for on with alpha beta gamma delta index indexes indexing indexed "
    "parse parser parsers parsing library libraries glass class running runs ran state machine "
    "machines release releases notes note quickly quick fox foxes café naïve straße STRASSE "
    "x1 2024 v2 e-mail co-op data_set Über über ДАННЫЕ данные"
).split()


def _ns_precision_supported() -> bool:
    with tempfile.TemporaryDirectory() as tmp:
        probe = Path(tmp, "probe")
        probe.write_bytes(b"x")
        wanted = BASE_SECONDS * 10**9 + 123_456_789
        os.utime(probe, ns=(wanted, wanted))
        return probe.stat().st_mtime_ns == wanted


NS_PRECISION = _ns_precision_supported()


def configs(spec: dict):
    """The same config for the agent's package and the reference."""
    kwargs = dict(stemming=spec.get("stemming", True), ignore=tuple(spec.get("ignore", ())))
    if "stopwords" in spec:
        kwargs["stopwords"] = frozenset(spec["stopwords"])
    return indexer.IndexConfig(**kwargs), refindexer.IndexConfig(**kwargs)


def stats_tuple(stats) -> tuple:
    return (stats.files_parsed, stats.files_reused, stats.files_removed, stats.full_rebuild)


class Tree:
    """A directory tree with deterministic, strictly increasing mtimes."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.tick = 0
        root.mkdir(parents=True, exist_ok=True)

    def next_mtime(self) -> int:
        self.tick += 1
        ns = (self.tick * 7_919_113) % 10**9 if NS_PRECISION else 0
        return (BASE_SECONDS + self.tick) * 10**9 + ns

    def write(self, rel: str, data, mtime: int | None = None) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data.encode("utf-8") if isinstance(data, str) else data)
        mtime = self.next_mtime() if mtime is None else mtime
        os.utime(path, ns=(mtime, mtime))

    def read(self, rel: str) -> bytes:
        return (self.root / rel).read_bytes()

    def mtime(self, rel: str) -> int:
        return (self.root / rel).stat().st_mtime_ns

    def touch(self, rel: str) -> None:
        mtime = self.next_mtime()
        os.utime(self.root / rel, ns=(mtime, mtime))

    def delete(self, rel: str) -> None:
        (self.root / rel).unlink()

    def rename(self, old: str, new: str) -> None:
        (self.root / new).parent.mkdir(parents=True, exist_ok=True)
        os.replace(self.root / old, self.root / new)


class Harness(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        base = Path(self.tmp.name)
        self.tree = Tree(base / "root")
        self.index = base / "out" / "index.json"
        self.ref_index = base / "ref" / "index.json"
        self.full_index = base / "full" / "index.json"
        for path in (self.index, self.ref_index, self.full_index):
            path.parent.mkdir()
        self.spec: dict = {}
        self.ref_docs: dict[str, str] | None = None
        self.ref_config = None

    def tearDown(self):
        self.tmp.cleanup()

    # -- helpers -----------------------------------------------------------------------

    def reference_build(self) -> bytes:
        _, ref_cfg = configs(self.spec)
        refindexer.build_index(self.tree.root, self.ref_index, ref_cfg)
        return self.ref_index.read_bytes()

    def expected_stats(self, new_docs: dict[str, str], new_config, forced: bool) -> tuple:
        if forced or self.ref_docs is None or self.ref_config != new_config:
            return (len(new_docs), 0, 0, True)
        common = set(new_docs) & set(self.ref_docs)
        reused = sum(1 for p in common if new_docs[p] == self.ref_docs[p])
        return (len(new_docs) - reused, reused, len(set(self.ref_docs) - set(new_docs)), False)

    def build(self, context: str = "", forced: bool = False, incremental: bool = True, check_full: bool = True):
        """Build incrementally with the agent's code and check bytes and stats."""
        cfg, _ = configs(self.spec)
        stats = indexer.build_index(self.tree.root, self.index, cfg, incremental=incremental)
        expected_bytes = self.reference_build()
        ref = json.loads(expected_bytes)
        new_docs = {d["path"]: d["sha256"] for d in ref["docs"]}
        expected = self.expected_stats(new_docs, ref["config"], forced or not incremental)
        self.assertEqual(
            self.index.read_bytes().decode("utf-8", "replace"),
            expected_bytes.decode("utf-8"),
            f"index bytes differ from a full build {context}",
        )
        self.assertEqual(stats_tuple(stats), expected, f"(files_parsed, files_reused, files_removed, full_rebuild) {context}")
        if check_full:
            indexer.build_index(self.tree.root, self.full_index, cfg)
            self.assertEqual(self.full_index.read_bytes(), expected_bytes, f"full build differs from reference {context}")
        self.ref_docs, self.ref_config = new_docs, ref["config"]
        return stats


def sentence(rng: random.Random, n: int | None = None) -> str:
    words = [rng.choice(VOCAB) for _ in range(n or rng.randint(1, 12))]
    return " ".join(words) + rng.choice(["", "\n", ".\n", "!\n"])


class TargetedTest(Harness):
    def seed_tree(self):
        self.tree.write("a.txt", "the quick brown fox\n")
        self.tree.write("b.md", "release notes for the parser\n")
        self.tree.write("docs/c.txt", "state machines and parsers\n")
        self.tree.write("docs/d.txt", "unique zebra words\n")
        self.tree.write("script.py", "not indexed\n")

    def test_stats_type_is_exported(self):
        self.assertTrue(hasattr(indexer, "IndexStats"), "indexer.IndexStats is not exported")
        self.seed_tree()
        stats = indexer.build_index(self.tree.root, self.index)
        self.assertIsInstance(stats, indexer.IndexStats)

    def test_full_build_stats(self):
        self.seed_tree()
        self.build("full build", incremental=False)

    def test_incremental_false_ignores_previous_index(self):
        self.seed_tree()
        self.build("first")
        self.tree.touch("a.txt")
        self.build("non-incremental rebuild", incremental=False)

    def test_first_incremental_build_without_index_is_full(self):
        self.seed_tree()
        self.assertEqual(stats_tuple(self.build("first build")), (4, 0, 0, True))

    def test_no_changes_reuses_everything(self):
        self.seed_tree()
        self.build("first")
        self.assertEqual(stats_tuple(self.build("unchanged tree")), (0, 4, 0, False))

    def test_touch_without_change_updates_stat_only(self):
        self.seed_tree()
        self.build("first")
        self.tree.touch("b.md")
        self.tree.touch("docs/c.txt")
        self.assertEqual(stats_tuple(self.build("after touch")), (0, 4, 0, False))
        doc = json.loads(self.index.read_bytes())["docs"][1]
        self.assertEqual(doc["mtime_ns"], self.tree.mtime("b.md"))

    def test_rewrite_same_content_new_mtime_is_reused(self):
        self.seed_tree()
        self.build("first")
        self.tree.write("a.txt", "the quick brown fox\n")
        self.assertEqual(stats_tuple(self.build("same content rewritten")), (0, 4, 0, False))

    def test_size_change_with_same_mtime_is_detected(self):
        self.seed_tree()
        self.build("first")
        old = self.tree.mtime("a.txt")
        self.tree.write("a.txt", "the quick brown fox jumps\n", mtime=old)
        self.assertEqual(stats_tuple(self.build("size changed, mtime kept")), (1, 3, 0, False))

    def test_same_size_new_mtime_changed_content_is_parsed(self):
        self.seed_tree()
        self.build("first")
        self.tree.write("a.txt", "the quick brown cat\n")
        self.assertEqual(stats_tuple(self.build("same size, new content")), (1, 3, 0, False))
        self.assertEqual(indexer.search_file(self.index, "cat"), ["a.txt"])
        self.assertEqual(indexer.search_file(self.index, "fox"), [])

    @unittest.skipUnless(NS_PRECISION, "filesystem does not keep nanosecond mtimes")
    def test_mtime_compared_in_nanoseconds(self):
        self.seed_tree()
        self.build("first")
        old = self.tree.mtime("a.txt")
        self.tree.write("a.txt", "the quick brown cat\n", mtime=old + 1)
        self.assertEqual(stats_tuple(self.build("mtime differs by 1ns")), (1, 3, 0, False))

    def test_unchanged_stat_is_trusted(self):
        self.seed_tree()
        self.build("first")
        old = self.tree.mtime("a.txt")
        self.tree.write("a.txt", "the quick brown cat\n", mtime=old)
        cfg, _ = configs(self.spec)
        stats = indexer.build_index(self.tree.root, self.index, cfg, incremental=True)
        self.assertEqual(stats_tuple(stats), (0, 4, 0, False), "a file with unchanged size and mtime_ns must be reused")
        self.assertEqual(indexer.search_file(self.index, "fox"), ["a.txt"], "a file with unchanged stat must not be re-read")

    def test_reverted_content_is_reused(self):
        self.seed_tree()
        self.build("first")
        self.tree.write("a.txt", "something else entirely\n")
        self.tree.write("a.txt", "the quick brown fox\n")
        self.assertEqual(stats_tuple(self.build("content reverted")), (0, 4, 0, False))

    def test_delete_drops_terms_and_renumbers(self):
        self.seed_tree()
        self.build("first")
        self.tree.delete("b.md")
        self.tree.delete("docs/d.txt")
        self.assertEqual(stats_tuple(self.build("after deletes")), (0, 2, 2, False))
        data = json.loads(self.index.read_bytes())
        self.assertNotIn("zebra", data["postings"])
        self.assertEqual(data["stats"], {"documents": 2, "tokens": 6})

    def test_new_file_sorting_first_renumbers_documents(self):
        self.seed_tree()
        self.build("first")
        self.tree.write("0-intro.txt", "quick intro to the parser\n")
        self.assertEqual(stats_tuple(self.build("new first file")), (1, 4, 0, False))
        self.assertEqual(indexer.search_file(self.index, "quick"), ["0-intro.txt", "a.txt"])

    def test_rename_is_remove_plus_parse(self):
        self.seed_tree()
        self.build("first")
        self.tree.rename("docs/d.txt", "archive/d.txt")
        self.assertEqual(stats_tuple(self.build("after rename")), (1, 3, 1, False))

    def test_directory_rename(self):
        self.seed_tree()
        self.build("first")
        self.tree.rename("docs", "manual")
        self.assertEqual(stats_tuple(self.build("after directory rename")), (2, 2, 2, False))

    def test_rename_onto_existing_path(self):
        self.seed_tree()
        self.build("first")
        self.tree.rename("docs/c.txt", "a.txt")
        self.assertEqual(stats_tuple(self.build("rename over a.txt")), (1, 2, 1, False))

    def test_copy_with_same_content(self):
        self.seed_tree()
        self.build("first")
        self.tree.write("copy.txt", self.tree.read("a.txt"))
        self.assertEqual(stats_tuple(self.build("copy of a.txt")), (1, 4, 0, False))

    def test_changes_to_unindexed_files_are_invisible(self):
        self.spec = {"ignore": ["build"]}
        self.seed_tree()
        self.build("first")
        self.tree.write("build/out.txt", "ignored\n")
        self.tree.write(".hidden/x.txt", "hidden\n")
        self.tree.write("notes.rst", "wrong suffix\n")
        self.tree.write("script.py", "changed\n")
        self.assertEqual(stats_tuple(self.build("unindexed changes")), (0, 4, 0, False))

    def test_stopwords_change_forces_full_build(self):
        self.seed_tree()
        self.build("first")
        self.spec = {"stopwords": ["the", "for"]}
        self.assertEqual(stats_tuple(self.build("stopwords changed")), (4, 0, 0, True))

    def test_stemming_change_forces_full_build(self):
        self.seed_tree()
        self.build("first")
        self.spec = {"stemming": False}
        self.assertEqual(stats_tuple(self.build("stemming off")), (4, 0, 0, True))

    def test_config_ignore_change_forces_full_build(self):
        self.seed_tree()
        self.build("first")
        self.spec = {"ignore": ["*.md"]}
        self.assertEqual(stats_tuple(self.build("ignore rule added")), (3, 0, 0, True))

    def test_indexignore_change_forces_full_build(self):
        self.seed_tree()
        self.build("first")
        self.tree.write(".indexignore", "# drafts\nnothing-matches\n")
        self.assertEqual(stats_tuple(self.build(".indexignore created")), (4, 0, 0, True))
        self.assertEqual(stats_tuple(self.build("unchanged")), (0, 4, 0, False))
        self.tree.write(".indexignore", "# drafts\nnothing-matches\n\n")
        self.assertEqual(stats_tuple(self.build("blank line added")), (0, 4, 0, False))
        self.tree.write(".indexignore", "docs/d.txt\n")
        self.assertEqual(stats_tuple(self.build(".indexignore changed")), (3, 0, 0, True))

    def test_ignore_rule_order_matters(self):
        self.spec = {"ignore": ["x*"]}
        self.seed_tree()
        self.tree.write(".indexignore", "y*\n")
        self.build("first")
        self.spec = {"ignore": ["y*"]}
        self.tree.write(".indexignore", "x*\n")
        self.assertEqual(stats_tuple(self.build("rules reordered")), (4, 0, 0, True))

    def tamper(self, mutate):
        self.seed_tree()
        self.build("first")
        self.tree.touch("a.txt")
        mutate()
        self.assertEqual(stats_tuple(self.build("after tampering", forced=True)), (4, 0, 0, True))

    def test_other_version_forces_full_build(self):
        def older():
            data = json.loads(self.index.read_bytes())
            data["version"] = 1
            self.index.write_text(json.dumps(data), encoding="utf-8")
        self.tamper(older)

    def test_corrupt_index_forces_full_build(self):
        self.tamper(lambda: self.index.write_bytes(b'{"format": "tinyindex", "vers'))

    def test_foreign_json_forces_full_build(self):
        self.tamper(lambda: self.index.write_text('{"hello": "world"}', encoding="utf-8"))

    def test_malformed_index_forces_full_build(self):
        def malformed():
            data = json.loads(self.index.read_bytes())
            del data["docs"]
            self.index.write_text(json.dumps(data), encoding="utf-8")
        self.tamper(malformed)

    def test_invalid_utf8_and_unicode_names(self):
        self.seed_tree()
        self.tree.write("ünïcode/naïve café.txt", "naïve café\n")
        self.tree.write("bin.txt", b"caf\xe9 \xff\xfe data\n")
        self.build("first")
        self.tree.write("bin.txt", b"caf\xe9 \xff\xfe more data\n")
        self.assertEqual(stats_tuple(self.build("binary changed")), (1, 5, 0, False))

    def test_empty_tree_and_emptied_tree(self):
        self.build("empty tree")
        self.seed_tree()
        self.build("seeded")
        for rel in ["a.txt", "b.md", "docs/c.txt", "docs/d.txt"]:
            self.tree.delete(rel)
        self.assertEqual(stats_tuple(self.build("emptied")), (0, 0, 4, False))

    def run_cli(self, *args: str) -> subprocess.CompletedProcess:
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        return subprocess.run([sys.executable, "-m", "indexer", *args], capture_output=True, text=True, env=env, timeout=60)

    def test_cli_output(self):
        self.seed_tree()
        index = str(self.index)
        result = self.run_cli("build", str(self.tree.root), "--index", index)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"indexed 4 files into {index} (parsed 4, reused 0, removed 0)\n")
        self.tree.delete("b.md")
        self.tree.write("e.txt", "new\n")
        result = self.run_cli("build", str(self.tree.root), "--index", index, "--incremental")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"indexed 4 files into {index} (parsed 1, reused 3, removed 1)\n")


class RandomizedTest(Harness):
    """Random mutation sequences; every build is checked against the reference."""

    STEPS = 14

    def all_files(self) -> list[str]:
        root = self.tree.root
        return sorted(
            p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file() and p.name != ".indexignore"
        )

    def random_path(self, rng: random.Random) -> str:
        dirs = ["", "", "docs/", "docs/api/", "notes/", "build/", ".cache/", "src/", "notes/old/"]
        names = ["readme", "intro", "guide", "a", "b", "z", "0-start", "Zeta", "über", "changelog", "x.tmp"]
        suffix = rng.choice([".txt", ".txt", ".md", ".md", ".py", ".rst"])
        return rng.choice(dirs) + rng.choice(names) + str(rng.randint(0, 3)) + suffix

    def mutate(self, rng: random.Random, log: list[str]) -> bool:
        """Apply one random mutation; returns True when the agent's index was tampered with."""
        files = self.all_files()
        op = rng.choice(
            ["create", "create", "modify", "modify", "same_size", "touch", "touch", "rewrite_same", "grow_keep_mtime",
             "delete", "rename", "rename_dir", "copy", "revert", "config", "indexignore", "tamper"]
        )
        if not files and op not in ("config", "indexignore", "tamper"):
            op = "create"
        rel = rng.choice(files) if files else ""
        if op == "create":
            rel = self.random_path(rng)
            self.tree.write(rel, sentence(rng))
        elif op == "modify":
            self.tree.write(rel, sentence(rng) + sentence(rng))
        elif op == "same_size":
            data = bytearray(self.tree.read(rel))
            if data:
                data[rng.randrange(len(data))] = ord(rng.choice("qwxz"))
            self.tree.write(rel, bytes(data))
        elif op == "touch":
            self.tree.touch(rel)
        elif op == "rewrite_same":
            mtime = self.tree.mtime(rel)
            self.tree.write(rel, self.tree.read(rel), mtime=mtime)
        elif op == "grow_keep_mtime":
            mtime = self.tree.mtime(rel)
            self.tree.write(rel, self.tree.read(rel) + b" more words", mtime=mtime)
        elif op == "delete":
            self.tree.delete(rel)
        elif op == "rename":
            new = self.random_path(rng)
            self.tree.rename(rel, new)
            rel = f"{rel} -> {new}"
        elif op == "rename_dir":
            parent = rel.rsplit("/", 1)[0] if "/" in rel else ""
            if parent and not (self.tree.root / ("moved-" + parent.replace("/", "-"))).exists():
                new = "moved-" + parent.replace("/", "-")
                self.tree.rename(parent, new)
                rel = f"{parent}/ -> {new}/"
            else:
                self.tree.delete(rel)
                op = "delete"
        elif op == "copy":
            new = self.random_path(rng)
            self.tree.write(new, self.tree.read(rel))
            rel = f"{rel} -> {new}"
        elif op == "revert":
            self.tree.write(rel, b"the quick brown fox\n")
        elif op == "config":
            self.spec = rng.choice([
                {}, {}, {"stemming": False}, {"stopwords": ["the", "and", "notes"]},
                {"ignore": ["build"]}, {"ignore": ["build", "*.md"]}, {"ignore": ["docs/api/*"]},
            ])
            rel = json.dumps(self.spec)
        elif op == "indexignore":
            text = rng.choice(["", "build\n", "# none\n", "notes\n", "*.tmp*\nbuild\n", "build\n*.tmp*\n"])
            self.tree.write(".indexignore", text)
            rel = repr(text)
        elif op == "tamper":
            if self.index.exists():
                self.index.write_bytes(rng.choice([b"", b"[]", b'{"format":"tinyindex","version":3}']))
                log.append("tamper index")
                return True
            op = "noop"
        log.append(f"{op} {rel}")
        return False

    def run_sequence(self, seed: int) -> None:
        rng = random.Random(seed)
        for _ in range(rng.randint(3, 10)):
            self.tree.write(self.random_path(rng), sentence(rng))
        log: list[str] = []
        self.build(f"(seed {seed}, initial build)")
        for step in range(self.STEPS):
            tampered = False
            for _ in range(rng.randint(1, 4)):
                tampered |= self.mutate(rng, log)
            context = f"(seed {seed}, step {step}, ops so far: {'; '.join(log[-6:])})"
            self.build(context, forced=tampered, check_full=step % 3 == 0)
        queries = ["quick", "notes OR parser", '"quick brown"', "(state OR data) AND the", "über", "e-mail"]
        _, ref_cfg = configs(self.spec)
        ref_index, ref_meta = refindexer.load_index(self.ref_index)
        for query in queries:
            self.assertEqual(
                indexer.search_file(self.index, query),
                refindexer.search(ref_index, query, ref_meta.config()),
                f"query {query!r} (seed {seed})",
            )


def _make_random_test(seed: int):
    def test(self):
        self.run_sequence(seed)
    return test


for _seed in range(40):
    setattr(RandomizedTest, f"test_random_sequence_{_seed:02d}", _make_random_test(1000 + _seed))


if __name__ == "__main__":
    unittest.main()
