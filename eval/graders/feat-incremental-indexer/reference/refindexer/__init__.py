"""tinyindex: a small full-text indexer for directories of text and Markdown files."""

from .build import IndexStats, build_index, parse_file
from .config import DEFAULT_STOPWORDS, FORMAT_VERSION, IndexConfig
from .index import Document, InvertedIndex
from .query import QuerySyntaxError, search, search_file
from .storage import IndexFormatError, IndexMeta, encode, load_index, save_index
from .tokenizer import stem, tokenize
from .walker import load_ignore_rules, walk

__all__ = [
    "DEFAULT_STOPWORDS",
    "FORMAT_VERSION",
    "Document",
    "IndexConfig",
    "IndexFormatError",
    "IndexMeta",
    "IndexStats",
    "InvertedIndex",
    "QuerySyntaxError",
    "build_index",
    "encode",
    "load_ignore_rules",
    "load_index",
    "parse_file",
    "save_index",
    "search",
    "search_file",
    "stem",
    "tokenize",
    "walk",
]
