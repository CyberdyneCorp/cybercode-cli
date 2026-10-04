"""Boolean and phrase queries over an InvertedIndex.

Grammar (keywords are case-sensitive; anything else is a word):

    query   := or_expr
    or_expr := and_expr ("OR" and_expr)*
    and_expr:= atom (["AND"] atom)*          # adjacency means AND
    atom    := WORD | '"' words '"' | "(" or_expr ")"

A WORD or a quoted phrase is run through the same tokenizer as the documents. A single
term matches the documents containing it; several terms (a phrase, or a word such as
"e-mail" that splits in two) match documents where they occur at the same relative
positions as in the query. An atom that yields no terms (only stopwords) places no
constraint on an AND and matches nothing on its own. Results are paths in ascending order.
"""

from __future__ import annotations

import os

from .config import IndexConfig
from .index import InvertedIndex
from .storage import load_index
from .tokenizer import tokenize


class QuerySyntaxError(ValueError):
    pass


def _lex(query: str) -> list[tuple[str, str]]:
    tokens, i = [], 0
    while i < len(query):
        ch = query[i]
        if ch.isspace():
            i += 1
        elif ch in "()":
            tokens.append((ch, ch))
            i += 1
        elif ch == '"':
            end = query.find('"', i + 1)
            if end < 0:
                raise QuerySyntaxError("unterminated phrase")
            tokens.append(("phrase", query[i + 1 : end]))
            i = end + 1
        else:
            start = i
            while i < len(query) and not query[i].isspace() and query[i] not in '()"':
                i += 1
            word = query[start:i]
            tokens.append((word, word) if word in ("AND", "OR") else ("word", word))
    return tokens


class _Parser:
    def __init__(self, tokens: list[tuple[str, str]]) -> None:
        self.tokens = tokens
        self.pos = 0

    def peek(self) -> str | None:
        return self.tokens[self.pos][0] if self.pos < len(self.tokens) else None

    def take(self) -> tuple[str, str]:
        token = self.tokens[self.pos]
        self.pos += 1
        return token

    def parse(self):
        if not self.tokens:
            raise QuerySyntaxError("empty query")
        node = self.or_expr()
        if self.peek() is not None:
            raise QuerySyntaxError(f"unexpected {self.tokens[self.pos][1]!r}")
        return node

    def or_expr(self):
        nodes = [self.and_expr()]
        while self.peek() == "OR":
            self.take()
            nodes.append(self.and_expr())
        return ("or", nodes) if len(nodes) > 1 else nodes[0]

    def and_expr(self):
        nodes = [self.atom()]
        while self.peek() in ("AND", "word", "phrase", "("):
            if self.peek() == "AND":
                self.take()
            nodes.append(self.atom())
        return ("and", nodes) if len(nodes) > 1 else nodes[0]

    def atom(self):
        kind = self.peek()
        if kind in ("word", "phrase"):
            return ("text", self.take()[1])
        if kind == "(":
            self.take()
            node = self.or_expr()
            if self.peek() != ")":
                raise QuerySyntaxError("missing ')'")
            self.take()
            return node
        raise QuerySyntaxError("expected a word, phrase or '('" if kind is None else f"unexpected {kind!r}")


def _match_text(index: InvertedIndex, text: str, config: IndexConfig) -> set[int] | None:
    terms = list(tokenize(text, config))
    if not terms:
        return None
    first_term, first_offset = terms[0]
    matches = set()
    for doc_id in index.doc_ids(first_term):
        for start in index.positions(first_term, doc_id):
            base = start - first_offset
            if all(base + offset in index.positions(term, doc_id) for term, offset in terms[1:]):
                matches.add(doc_id)
                break
    return matches


def _evaluate(node, index: InvertedIndex, config: IndexConfig) -> set[int] | None:
    kind, value = node
    if kind == "text":
        return _match_text(index, value, config)
    results = [_evaluate(child, index, config) for child in value]
    if kind == "or":
        return set().union(*(r for r in results if r is not None))
    constrained = [r for r in results if r is not None]
    if not constrained:
        return None
    return set.intersection(*constrained)


def search(index: InvertedIndex, query: str, config: IndexConfig) -> list[str]:
    matches = _evaluate(_Parser(_lex(query)).parse(), index, config) or set()
    return sorted(index.path(doc_id) for doc_id in matches)


def search_file(index_path: str | os.PathLike, query: str) -> list[str]:
    """Load the index at `index_path` and search it with the settings it was built with."""
    index, meta = load_index(index_path)
    return search(index, query, meta.config())
