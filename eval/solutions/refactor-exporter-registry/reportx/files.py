"""File names for exported reports."""

import re
from pathlib import PurePath

from .exporters import available_formats, get_exporter


def extension(fmt: str) -> str:
    """The file extension for a format, including the dot."""
    return get_exporter(fmt).extension


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
    for name in available_formats():
        exporter = get_exporter(name)
        if suffix in {exporter.extension, *("." + key for key in (exporter.name, *exporter.aliases))}:
            return name
    return None
