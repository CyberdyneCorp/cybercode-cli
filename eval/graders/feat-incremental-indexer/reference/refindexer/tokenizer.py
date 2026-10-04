"""Turn document or query text into (term, position) pairs.

Text is case-folded and split into words: maximal runs of letters and digits (`[^\\W_]+`).
Every word gets a position, its 0-based ordinal among all words of the text. Stopwords are
then dropped (their positions are skipped, not reused, so phrase queries see the gap) and,
when stemming is enabled, the remaining words are reduced by `stem`.
"""

from __future__ import annotations

import re
from collections.abc import Iterator

from .config import IndexConfig

WORD_RE = re.compile(r"[^\W_]+")

# (suffix, replacement, minimum length of the remaining stem); first match wins.
_SUFFIX_RULES = (
    ("sses", "ss", 2),
    ("ies", "y", 2),
    ("ing", "", 3),
    ("ed", "", 3),
    ("ly", "", 3),
    ("s", "", 3),
)


def stem(word: str) -> str:
    """A deliberately small suffix stripper: "indexes" -> "indexe", "running" -> "runn"."""
    for suffix, replacement, min_stem in _SUFFIX_RULES:
        if word.endswith(suffix):
            base = word[: len(word) - len(suffix)]
            if len(base) < min_stem or (suffix == "s" and word.endswith("ss")):
                return word
            return base + replacement
    return word


def words(text: str) -> list[str]:
    return WORD_RE.findall(text.casefold())


def tokenize(text: str, config: IndexConfig) -> Iterator[tuple[str, int]]:
    for position, word in enumerate(words(text)):
        if word in config.stopwords:
            continue
        yield (stem(word) if config.stemming else word), position


def group_positions(tokens: Iterator[tuple[str, int]]) -> dict[str, list[int]]:
    """Collect the ascending positions of every term."""
    grouped: dict[str, list[int]] = {}
    for term, position in tokens:
        grouped.setdefault(term, []).append(position)
    return grouped
