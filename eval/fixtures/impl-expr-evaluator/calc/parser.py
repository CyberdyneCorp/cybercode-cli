"""Parser for calc (see SPEC.md, Grammar). Not implemented yet."""

from .lexer import tokenize


def parse(source: str):
    tokens = tokenize(source)
    raise NotImplementedError("parser not implemented")
