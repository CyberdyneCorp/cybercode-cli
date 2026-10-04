"""Index configuration and format constants."""

from __future__ import annotations

from dataclasses import dataclass, field

FORMAT_NAME = "tinyindex"
FORMAT_VERSION = 2

DEFAULT_STOPWORDS = frozenset(
    {"a", "an", "and", "are", "as", "at", "be", "by", "for", "in", "is", "it", "of", "on", "or", "the", "to", "with"}
)


@dataclass(frozen=True)
class IndexConfig:
    """Settings that determine what is indexed and how text becomes terms.

    `stopwords` are compared after case folding; `stemming` enables the suffix stemmer in
    tokenizer.py; `ignore` holds extra ignore patterns (see walker.py) that are applied
    before the patterns read from the root's `.indexignore` file.
    """

    stopwords: frozenset[str] = DEFAULT_STOPWORDS
    stemming: bool = True
    ignore: tuple[str, ...] = field(default_factory=tuple)

    def __post_init__(self) -> None:
        object.__setattr__(self, "stopwords", frozenset(word.casefold() for word in self.stopwords))
        object.__setattr__(self, "ignore", tuple(self.ignore))
