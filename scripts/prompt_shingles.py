#!/usr/bin/env python3
"""Build or check the provenance fixture for our base prompts.

Our base prompts (crates/cyber-core/prompts/base/*.md), any patch grammar
(crates/**/*.lark) and the model-facing strings of our tools (string literals of
crates/cyber-tools/src/tools/*.rs) are written clean-room. To keep them that way, the
fixture lists short hashes of every 12-word run ("shingle") of the reference product's
text assets: every .md, .lark and .txt under its codex-rs tree, the text files of any
assets, templates or prompts folder, and each string literal of its non-test Rust
sources. Grammars use short tokens, so a second fixture holds the 6-word runs of the
reference .lark files. A test fails if any run of our assets hashes to one of them.
Only hashes are stored; the reference text is never vendored.

Normalization, mirrored by crates/cyber-core/tests/prompt_provenance.rs:
  * words are maximal runs of ASCII letters and digits, lowercased; everything else
    (punctuation, Markdown, non-ASCII) separates words;
  * a shingle is 12 consecutive words of one file joined by single spaces;
  * its hash is the first 12 hex digits of SHA-256 over the UTF-8 bytes;
  * a .rs file contributes each string literal before its first `#[cfg(test)]` as its
    own text (see `rust_literals`); a backslash escape separates words.

Usage:
  python3 scripts/prompt_shingles.py generate <reference checkout>   # rewrite the fixture
  python3 scripts/prompt_shingles.py check                           # check our assets
Exit code 1 when `check` finds a match.
"""
from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROMPTS = ROOT / "crates" / "cyber-core" / "prompts" / "base"
FIXTURES = ROOT / "crates" / "cyber-core" / "tests" / "fixtures"
FIXTURE = FIXTURES / "reference_prompt_shingles.txt"
GRAMMAR_FIXTURE = FIXTURES / "reference_grammar_shingles.txt"
TOOLS = ROOT / "crates" / "cyber-tools" / "src" / "tools"
WIDTH = 12
GRAMMAR_WIDTH = 6
WORD_RE = re.compile(r"[A-Za-z0-9]+")


def words(text: str) -> list[str]:
    return [word.lower() for word in WORD_RE.findall(text)]


def shingle_hash(run: list[str]) -> str:
    return hashlib.sha256(" ".join(run).encode()).hexdigest()[:12]


def shingles(text: str, width: int = WIDTH) -> set[str]:
    run = words(text)
    return {shingle_hash(run[i : i + width]) for i in range(len(run) - width + 1)}


def is_ident(char: str) -> bool:
    return char.isascii() and (char.isalnum() or char == "_")


def rust_literals(source: str) -> list[str]:
    """The contents of the string literals before the first `#[cfg(test)]`, skipping
    `//` comments and char literals. Mirrored by `rust_literals` in the Rust test."""
    text = source.split("#[cfg(test)]", 1)[0]
    found, i, n = [], 0, len(text)
    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = n if end < 0 else end
        elif text[i] == "'":
            i = char_literal_end(text, i)
        elif text[i] == "r" and (i == 0 or not is_ident(text[i - 1])) and raw_opener(text, i):
            hashes = raw_opener(text, i) - 2
            start = i + hashes + 2
            end = text.find('"' + "#" * hashes, start)
            end = n if end < 0 else end
            found.append(text[start:end])
            i = end + hashes + 1
        elif text[i] == '"':
            body, i = quoted(text, i + 1)
            found.append(body)
        else:
            i += 1
    return found


def char_literal_end(text: str, i: int) -> int:
    """Past a char literal at `i`, or past the quote of a lifetime."""
    if text[i + 1 : i + 2] == "\\":
        end = text.find("'", i + 3)
        return len(text) if end < 0 else end + 1
    return i + 3 if text[i + 2 : i + 3] == "'" else i + 1


def raw_opener(text: str, i: int) -> int:
    """The length of a raw string opener `r#*"` at `i`, or 0."""
    j = i + 1
    while j < len(text) and text[j] == "#":
        j += 1
    return j - i + 1 if text[j : j + 1] == '"' else 0


def quoted(text: str, i: int) -> tuple[str, int]:
    """A string literal's body from `i`, an escape as a space, and the index past it."""
    body = []
    while i < len(text) and text[i] != '"':
        if text[i] == "\\":
            body.append(" ")
            i += 2
        else:
            body.append(text[i])
            i += 1
    return "".join(body), i + 1


def texts(path: Path) -> list[str]:
    """The texts of one file: a Rust file's string literals, else the whole file."""
    content = path.read_text(encoding="utf-8", errors="replace")
    return rust_literals(content) if path.suffix == ".rs" else [content]


# Text assets of the reference checkout that can hold prompt, grammar or tool text.
SOURCE_TREE = "codex-rs"
SOURCE_SUFFIXES = {".md", ".lark", ".txt"}
ASSET_DIRS = {"assets", "templates", "prompts"}
ASSET_SUFFIXES = SOURCE_SUFFIXES | {".yaml", ".yml", ".toml", ".json"}
SKIPPED_DIRS = {"node_modules", "target", ".git"}


def is_reference_asset(relative: Path) -> bool:
    """Every .md, .lark and .txt under the source tree and its non-test Rust sources,
    plus the text files of any assets, templates or prompts folder elsewhere."""
    folders = [part.lower() for part in relative.parts[:-1]]
    if SKIPPED_DIRS.intersection(folders):
        return False
    suffix = relative.suffix.lower()
    in_tree = folders[:1] == [SOURCE_TREE]
    if in_tree and suffix in SOURCE_SUFFIXES:
        return True
    if in_tree and suffix == ".rs":
        return "tests" not in folders and not relative.stem.endswith("tests")
    return suffix in ASSET_SUFFIXES and bool(ASSET_DIRS.intersection(folders))


def reference_files(checkout: Path) -> list[Path]:
    return [
        path
        for path in sorted(checkout.rglob("*"))
        if path.is_file() and is_reference_asset(path.relative_to(checkout))
    ]


def our_assets() -> list[Path]:
    """The model-facing text we ship: the base prompts, any grammar file, and the tool
    modules' string literals."""
    crates = ROOT / "crates"
    grammars = [
        path
        for path in crates.rglob("*.lark")
        if not SKIPPED_DIRS.intersection(path.relative_to(crates).parts)
    ]
    return sorted(PROMPTS.glob("*.md")) + sorted(grammars) + sorted(TOOLS.glob("*.rs"))


def generate(checkout: Path) -> int:
    files = reference_files(checkout)
    if not files:
        print(f"no text assets under {checkout}", file=sys.stderr)
        return 1
    hashes: set[str] = set()
    grammar: set[str] = set()
    for path in files:
        for text in texts(path):
            hashes |= shingles(text)
        if path.suffix == ".lark":
            grammar |= shingles(path.read_text(encoding="utf-8"), GRAMMAR_WIDTH)
    lark = sum(path.suffix == ".lark" for path in files)
    write_fixture(FIXTURE, hashes, WIDTH, f"{len(files)} reference text assets (prompts, templates, grammars, tool text)")
    write_fixture(GRAMMAR_FIXTURE, grammar, GRAMMAR_WIDTH, f"{lark} reference grammars")
    return 0


def write_fixture(path: Path, hashes: set[str], width: int, source: str) -> None:
    header = (
        f"# SHA-256/12 of each normalized {width}-word shingle of {source}.\n"
        "# Generated by scripts/prompt_shingles.py; hashes only.\n"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(header + "".join(f"{h}\n" for h in sorted(hashes)))
    print(f"{len(hashes)} shingles -> {path.relative_to(ROOT)}")


def fixture_hashes(path: Path = FIXTURE) -> set[str]:
    lines = path.read_text().splitlines()
    return {line for line in lines if line and not line.startswith("#")}


def matches(
    known: set[str] | None = None,
    assets: list[Path] | None = None,
    grammar: set[str] | None = None,
) -> list[tuple[str, str]]:
    """(file, run) pairs of our assets whose hash is in a fixture: 12-word runs of
    every text, and 6-word runs of a grammar."""
    known = fixture_hashes() if known is None else known
    grammar = fixture_hashes(GRAMMAR_FIXTURE) if grammar is None else grammar
    found = []
    for path in our_assets() if assets is None else assets:
        checks = [(text, WIDTH, known) for text in texts(path)]
        if path.suffix == ".lark":
            checks.append((path.read_text(encoding="utf-8"), GRAMMAR_WIDTH, grammar))
        for text, width, hashes in checks:
            found.extend((path.name, run) for run in runs_in(text, width, hashes))
    return found


def runs_in(text: str, width: int, hashes: set[str]) -> list[str]:
    run = words(text)
    windows = (run[i : i + width] for i in range(len(run) - width + 1))
    return [" ".join(window) for window in windows if shingle_hash(window) in hashes]


def check() -> int:
    found = matches()
    for name, window in found:
        print(f"{name}: matches a reference prompt: {window!r}")
    return 1 if found else 0


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "generate":
        return generate(Path(argv[1]))
    if argv == ["check"]:
        return check()
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
