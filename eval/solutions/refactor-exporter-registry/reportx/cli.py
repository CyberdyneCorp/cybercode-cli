"""Command line interface: python3 -m reportx INPUT --format FORMAT [options]."""

import argparse
import sys
from pathlib import Path

from .batch import export_all, parse_format_list
from .dataset import DatasetError, load_dataset
from .exporters import available_formats, get_exporter
from .files import guess_format
from .render import render


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="reportx", description="Export a dataset (JSON) as a report.")
    parser.add_argument("input", nargs="?", help="dataset file (JSON object with title, columns, rows)")
    parser.add_argument("-f", "--format", dest="fmt",
                        help="output format, or a comma-separated list with --output-dir (see --list-formats); "
                             "guessed from --output when omitted")
    parser.add_argument("--no-header", action="store_true", help="leave out the column-name row")
    destination = parser.add_mutually_exclusive_group()
    destination.add_argument("-o", "--output", help="write to this file instead of stdout")
    destination.add_argument("--output-dir", help="write <slug of title><extension> into this directory")
    parser.add_argument("--list-formats", action="store_true", help="list the formats and exit")
    return parser


def list_formats() -> str:
    lines = []
    for name in available_formats():
        exporter = get_exporter(name)
        line = f"{exporter.name}\t{exporter.extension}\t{exporter.content_type}"
        if exporter.aliases:
            line += "\taliases: " + ", ".join(exporter.aliases)
        lines.append(line + "\n")
    return "".join(lines)


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.list_formats:
        _write_stdout(list_formats())
        return 0
    if args.input is None:
        parser.error("an input file is required")
    try:
        formats = _requested_formats(args)
    except ValueError as error:
        return _fail(str(error), 2)
    try:
        dataset = load_dataset(args.input)
    except OSError as error:
        return _fail(f"cannot read {args.input}: {error.strerror}", 1)
    except DatasetError as error:
        return _fail(f"{args.input}: {error}", 1)

    header = not args.no_header
    for fmt in formats:
        if args.no_header and not get_exporter(fmt).header_row:
            print(f"reportx: warning: --no-header is ignored for {fmt} output", file=sys.stderr)
    try:
        if args.output_dir is not None:
            paths = export_all(dataset, formats, args.output_dir, header)
        else:
            paths = [_export_one(dataset, formats[0], args.output, header)]
    except ValueError as error:
        return _fail(str(error), 2)
    except OSError as error:
        return _fail(f"cannot write {error.filename}: {error.strerror}", 1)
    rows = len(dataset.rows)
    for path in paths:
        if path is not None:
            _write_stdout(f"wrote {rows} {'row' if rows == 1 else 'rows'} to {path}\n")
    return 0


def _requested_formats(args) -> list[str]:
    if args.fmt is None:
        guessed = guess_format(args.output) if args.output is not None else None
        if guessed is None:
            raise ValueError("--format is required unless it can be guessed from --output")
        return [guessed]
    formats = parse_format_list(args.fmt)
    if len(formats) > 1 and args.output_dir is None:
        raise ValueError("several formats need --output-dir")
    return formats


def _export_one(dataset, fmt: str, output, header: bool):
    """Write to `output`, or to stdout when it is None (returns the path written, or None)."""
    text = render(dataset, fmt, header=header)
    if output is None:
        _write_stdout(text)
        return None
    with open(output, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)
    return output


def _write_stdout(text: str) -> None:
    sys.stdout.flush()
    sys.stdout.buffer.write(text.encode("utf-8"))
    sys.stdout.buffer.flush()


def _fail(message: str, status: int) -> int:
    print(f"reportx: error: {message}", file=sys.stderr)
    return status
