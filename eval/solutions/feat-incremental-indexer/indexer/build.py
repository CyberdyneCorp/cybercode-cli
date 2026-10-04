"""Build the on-disk index for a directory tree, from scratch or incrementally."""

from __future__ import annotations

import dataclasses
import hashlib
import os
from dataclasses import dataclass
from pathlib import Path

from .config import IndexConfig
from .index import Document, InvertedIndex
from .storage import IndexFormatError, IndexMeta, load_index, save_index
from .tokenizer import group_positions, tokenize
from .walker import load_ignore_rules, walk


@dataclass(frozen=True)
class IndexStats:
    """What a build did (see docs/INCREMENTAL.md)."""

    files_parsed: int
    files_reused: int
    files_removed: int
    full_rebuild: bool


def parse_file(root: str | os.PathLike, relpath: str, config: IndexConfig) -> tuple[Document, dict[str, list[int]]]:
    """Stat, hash and tokenize one file. Text is decoded as UTF-8 (invalid bytes replaced)."""
    path = Path(root, relpath)
    st = path.stat()
    raw = path.read_bytes()
    terms = group_positions(tokenize(raw.decode("utf-8", errors="replace"), config))
    length = sum(len(positions) for positions in terms.values())
    doc = Document(relpath, st.st_size, st.st_mtime_ns, hashlib.sha256(raw).hexdigest(), length)
    return doc, terms


def build_index(
    root: str | os.PathLike,
    index_path: str | os.PathLike,
    config: IndexConfig | None = None,
    incremental: bool = False,
) -> IndexStats:
    """Index every file under `root` and write the index to `index_path`.

    With `incremental=True` the index currently at `index_path` is reused where possible;
    the result is always byte-identical to a full build.
    """
    config = config or IndexConfig()
    rules = load_ignore_rules(root, config)
    previous = _compatible_previous(index_path, config, rules) if incremental else None
    if previous is None:
        index, stats = _full_build(root, config, rules)
    else:
        index, stats = _incremental_build(root, config, rules, previous)
    save_index(index_path, index, config, rules)
    return stats


def _full_build(root, config: IndexConfig, rules: tuple[str, ...]) -> tuple[InvertedIndex, IndexStats]:
    index = InvertedIndex()
    for relpath in walk(root, rules):
        index.add_document(*parse_file(root, relpath, config))
    return index, IndexStats(len(index), 0, 0, full_rebuild=True)


def _compatible_previous(index_path, config: IndexConfig, rules: tuple[str, ...]) -> InvertedIndex | None:
    """The previous index if it exists, loads, and was built with the same settings."""
    if not os.path.exists(index_path):
        return None
    try:
        index, meta = load_index(index_path)
    except IndexFormatError:
        return None
    return index if _same_settings(meta, config, rules) else None


def _same_settings(meta: IndexMeta, config: IndexConfig, rules: tuple[str, ...]) -> bool:
    return (
        meta.stemming == config.stemming
        and list(meta.stopwords) == sorted(config.stopwords)
        and list(meta.ignore) == list(rules)
    )


def _terms_by_doc(index: InvertedIndex) -> dict[int, dict[str, list[int]]]:
    """Invert the postings back into each document's term positions."""
    terms: dict[int, dict[str, list[int]]] = {doc_id: {} for doc_id in range(len(index))}
    for term, entries in index.postings.items():
        for doc_id, positions in entries:
            terms[doc_id][term] = positions
    return terms


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _incremental_build(
    root, config: IndexConfig, rules: tuple[str, ...], previous: InvertedIndex
) -> tuple[InvertedIndex, IndexStats]:
    previous_ids = {doc.path: doc_id for doc_id, doc in enumerate(previous.docs)}
    previous_terms = _terms_by_doc(previous)
    index = InvertedIndex()
    parsed = reused = 0
    walked = walk(root, rules)
    for relpath in walked:
        doc_id = previous_ids.get(relpath)
        if doc_id is not None:
            old = previous.docs[doc_id]
            st = Path(root, relpath).stat()
            if (st.st_size, st.st_mtime_ns) == (old.size, old.mtime_ns):
                index.add_document(old, previous_terms[doc_id])
                reused += 1
                continue
            if _sha256(Path(root, relpath)) == old.sha256:
                updated = dataclasses.replace(old, size=st.st_size, mtime_ns=st.st_mtime_ns)
                index.add_document(updated, previous_terms[doc_id])
                reused += 1
                continue
        index.add_document(*parse_file(root, relpath, config))
        parsed += 1
    removed = len(set(previous_ids) - set(walked))
    return index, IndexStats(parsed, reused, removed, full_rebuild=False)
