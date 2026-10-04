"""Command line interface: `python3 -m indexer build|search|stats ...`."""

from __future__ import annotations

import argparse
import sys

from .build import build_index
from .config import DEFAULT_STOPWORDS, IndexConfig
from .query import QuerySyntaxError, search_file
from .storage import IndexFormatError, load_index


def _config(args: argparse.Namespace) -> IndexConfig:
    stopwords = DEFAULT_STOPWORDS
    if args.stopwords is not None:
        stopwords = frozenset(w for w in args.stopwords.split(",") if w)
    return IndexConfig(stopwords=stopwords, stemming=not args.no_stem, ignore=tuple(args.ignore))


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="indexer", description="Tiny full-text indexer.")
    sub = parser.add_subparsers(dest="command", required=True)

    build = sub.add_parser("build", help="index a directory")
    build.add_argument("root")
    build.add_argument("--index", default="index.json", help="index file (default: index.json)")
    build.add_argument("--no-stem", action="store_true", help="disable stemming")
    build.add_argument("--stopwords", help="comma-separated stopword list replacing the default")
    build.add_argument("--ignore", action="append", default=[], help="extra ignore pattern (repeatable)")

    find = sub.add_parser("search", help="query an index")
    find.add_argument("query")
    find.add_argument("--index", default="index.json")

    stats = sub.add_parser("stats", help="describe an index")
    stats.add_argument("--index", default="index.json")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        if args.command == "build":
            count = build_index(args.root, args.index, _config(args))
            print(f"indexed {count} files into {args.index}")
        elif args.command == "search":
            for path in search_file(args.index, args.query):
                print(path)
        else:
            index, meta = load_index(args.index)
            print(f"documents: {len(index)}")
            print(f"tokens: {index.total_tokens}")
            print(f"terms: {len(index.postings)}")
            print(f"stemming: {'on' if meta.stemming else 'off'}")
    except (IndexFormatError, QuerySyntaxError) as exc:
        print(f"indexer: {exc}", file=sys.stderr)
        return 1
    return 0
