"""File names for exported reports."""

import re
from pathlib import PurePath

from .formats import normalize_format


def extension(fmt: str) -> str:
    """The file extension for a format, including the dot."""
    fmt = normalize_format(fmt)
    if fmt == "csv":
        return ".csv"
    elif fmt == "tsv":
        return ".tsv"
    elif fmt == "json":
        return ".json"
    elif fmt == "ndjson":
        return ".ndjson"
    elif fmt == "markdown":
        return ".md"
    elif fmt == "html":
        return ".html"
    else:
        return ".xml"


def slug(title: str) -> str:
    """Lowercase ASCII letters and digits, runs of anything else become one hyphen."""
    text = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    return text or "report"


def output_filename(title: str, fmt: str) -> str:
    return slug(title) + extension(fmt)


def guess_format(path: str) -> str | None:
    """The format whose name, alias or extension matches the file suffix (case-insensitive), or None.

    report.csv -> csv, report.MD -> markdown, report.markdown -> markdown, report.jsonl -> ndjson,
    report.htm -> html, report.txt -> None.
    """
    suffix = PurePath(path).suffix.lower()
    if suffix in (".csv",):
        return "csv"
    if suffix in (".tsv",):
        return "tsv"
    if suffix in (".json",):
        return "json"
    if suffix in (".ndjson", ".jsonl"):
        return "ndjson"
    if suffix in (".md", ".markdown"):
        return "markdown"
    if suffix in (".html", ".htm"):
        return "html"
    if suffix in (".xml",):
        return "xml"
    return None
