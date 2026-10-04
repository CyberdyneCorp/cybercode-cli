"""Render a TSV file (first line = headers) as a table.

    python3 -m termtable FILE [--max-width N] [--align l,r,c] [--wrap]

FILE may be `-` for standard input.
"""

from __future__ import annotations

import argparse
import sys

from .table import render_table

ALIGN_CODES = {"l": "left", "r": "right", "c": "center"}


def parse_align(value: str) -> list[str]:
    try:
        return [ALIGN_CODES[code.strip()] for code in value.split(",")]
    except KeyError as exc:
        raise argparse.ArgumentTypeError(f"unknown alignment {exc.args[0]!r} (use l, r or c)") from None


def read_rows(path: str) -> list[list[str]]:
    if path == "-":
        text = sys.stdin.read()
    else:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    return [line.split("\t") for line in text.splitlines() if line.strip()]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="termtable", description="Render a TSV file as a table.")
    parser.add_argument("file")
    parser.add_argument("--max-width", type=int, default=None)
    parser.add_argument("--align", type=parse_align, default=None)
    parser.add_argument("--wrap", action="store_true", help="wrap long cells instead of truncating")
    args = parser.parse_args(argv)
    try:
        rows = read_rows(args.file)
    except OSError as exc:
        print(f"termtable: cannot read {args.file}: {exc.strerror}", file=sys.stderr)
        return 1
    if not rows:
        return 0
    headers, body = rows[0], rows[1:]
    try:
        table = render_table(body, headers, max_width=args.max_width, align=args.align,
                             overflow="wrap" if args.wrap else "truncate")
    except ValueError as exc:
        print(f"termtable: {exc}", file=sys.stderr)
        return 2
    print(table)
    return 0


if __name__ == "__main__":
    sys.exit(main())
