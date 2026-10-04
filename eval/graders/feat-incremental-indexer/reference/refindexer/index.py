"""The in-memory inverted index.

Documents get consecutive integer ids in the order they are added; `build.py` adds them
in sorted path order, so the document with id N is the N-th path of the sorted walk.
Postings map each term to a list of (doc_id, positions) entries in ascending doc_id order.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Document:
    path: str  # relative POSIX path
    size: int  # st_size in bytes
    mtime_ns: int  # st_mtime_ns
    sha256: str  # hex digest of the raw file bytes
    length: int  # number of indexed tokens (stopwords excluded)


class InvertedIndex:
    def __init__(self) -> None:
        self.docs: list[Document] = []
        self.postings: dict[str, list[tuple[int, list[int]]]] = {}

    def __len__(self) -> int:
        return len(self.docs)

    @property
    def total_tokens(self) -> int:
        return sum(doc.length for doc in self.docs)

    def add_document(self, doc: Document, terms: dict[str, list[int]]) -> int:
        """Append `doc` with its term positions and return its id."""
        doc_id = len(self.docs)
        self.docs.append(doc)
        for term, positions in terms.items():
            self.postings.setdefault(term, []).append((doc_id, list(positions)))
        return doc_id

    def doc_ids(self, term: str) -> set[int]:
        return {doc_id for doc_id, _ in self.postings.get(term, ())}

    def positions(self, term: str, doc_id: int) -> list[int]:
        for candidate, positions in self.postings.get(term, ()):
            if candidate == doc_id:
                return positions
        return []

    def all_doc_ids(self) -> set[int]:
        return set(range(len(self.docs)))

    def path(self, doc_id: int) -> str:
        return self.docs[doc_id].path

    def find(self, path: str) -> Document | None:
        for doc in self.docs:
            if doc.path == path:
                return doc
        return None
