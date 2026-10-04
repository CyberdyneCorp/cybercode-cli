"""On-disk index format (see README.md, "Index file format").

The file is canonical JSON so that the same index always serializes to the same bytes:
UTF-8, keys sorted, no insignificant whitespace, non-ASCII characters written as-is and a
single trailing newline. Writes are atomic (temporary file + os.replace).
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from .config import FORMAT_NAME, FORMAT_VERSION, IndexConfig
from .index import Document, InvertedIndex


class IndexFormatError(ValueError):
    """The file is not a readable index of the current format version."""


@dataclass(frozen=True)
class IndexMeta:
    version: int
    stopwords: tuple[str, ...]
    stemming: bool
    ignore: tuple[str, ...]

    def config(self) -> IndexConfig:
        """The tokenizer settings the index was built with (ignore rules not included)."""
        return IndexConfig(stopwords=frozenset(self.stopwords), stemming=self.stemming)


def config_record(config: IndexConfig, rules: tuple[str, ...]) -> dict:
    return {"ignore": list(rules), "stemming": config.stemming, "stopwords": sorted(config.stopwords)}


def encode(index: InvertedIndex, config: IndexConfig, rules: tuple[str, ...]) -> bytes:
    data = {
        "format": FORMAT_NAME,
        "version": FORMAT_VERSION,
        "config": config_record(config, rules),
        "stats": {"documents": len(index), "tokens": index.total_tokens},
        "docs": [
            {"path": d.path, "size": d.size, "mtime_ns": d.mtime_ns, "sha256": d.sha256, "length": d.length}
            for d in index.docs
        ],
        "postings": {
            term: [[doc_id, positions] for doc_id, positions in entries]
            for term, entries in index.postings.items()
        },
    }
    text = json.dumps(data, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return (text + "\n").encode("utf-8")


def save_index(path: str | os.PathLike, index: InvertedIndex, config: IndexConfig, rules: tuple[str, ...]) -> None:
    path = Path(path)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_bytes(encode(index, config, rules))
    os.replace(tmp, path)


def load_index(path: str | os.PathLike) -> tuple[InvertedIndex, IndexMeta]:
    try:
        data = json.loads(Path(path).read_bytes().decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise IndexFormatError(f"cannot read index {path}: {exc}") from exc
    if not isinstance(data, dict) or data.get("format") != FORMAT_NAME:
        raise IndexFormatError(f"{path} is not a {FORMAT_NAME} index")
    if data.get("version") != FORMAT_VERSION:
        raise IndexFormatError(f"{path} has unsupported version {data.get('version')!r}")
    try:
        cfg = data["config"]
        meta = IndexMeta(FORMAT_VERSION, tuple(cfg["stopwords"]), bool(cfg["stemming"]), tuple(cfg["ignore"]))
        index = InvertedIndex()
        index.docs = [
            Document(d["path"], d["size"], d["mtime_ns"], d["sha256"], d["length"]) for d in data["docs"]
        ]
        index.postings = {
            term: [(int(doc_id), list(positions)) for doc_id, positions in entries]
            for term, entries in data["postings"].items()
        }
    except (KeyError, TypeError, ValueError) as exc:
        raise IndexFormatError(f"{path} is malformed: {exc}") from exc
    return index, meta
