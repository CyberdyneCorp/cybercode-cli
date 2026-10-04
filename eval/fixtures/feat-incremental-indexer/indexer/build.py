"""Build the on-disk index for a directory tree."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path

from .config import IndexConfig
from .index import Document, InvertedIndex
from .storage import save_index
from .tokenizer import group_positions, tokenize
from .walker import load_ignore_rules, walk


def parse_file(root: str | os.PathLike, relpath: str, config: IndexConfig) -> tuple[Document, dict[str, list[int]]]:
    """Stat, hash and tokenize one file. Text is decoded as UTF-8 (invalid bytes replaced)."""
    path = Path(root, relpath)
    st = path.stat()
    raw = path.read_bytes()
    terms = group_positions(tokenize(raw.decode("utf-8", errors="replace"), config))
    length = sum(len(positions) for positions in terms.values())
    doc = Document(relpath, st.st_size, st.st_mtime_ns, hashlib.sha256(raw).hexdigest(), length)
    return doc, terms


def build_index(root: str | os.PathLike, index_path: str | os.PathLike, config: IndexConfig | None = None) -> int:
    """Index every file under `root` and write the index to `index_path`.

    Returns the number of documents indexed.
    """
    config = config or IndexConfig()
    rules = load_ignore_rules(root, config)
    index = InvertedIndex()
    for relpath in walk(root, rules):
        doc, terms = parse_file(root, relpath, config)
        index.add_document(doc, terms)
    save_index(index_path, index, config, rules)
    return len(index)
