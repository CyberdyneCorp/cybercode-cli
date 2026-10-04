"""Exporting one dataset in several formats at once."""

from pathlib import Path

from .files import output_filename
from .formats import normalize_format
from .render import render


def parse_format_list(text: str) -> list[str]:
    """Split a comma-separated list of format names, normalize them and drop repeats (first wins).

    Raises ValueError for an empty list or an unknown format.
    """
    names = [part for part in (p.strip() for p in text.split(",")) if part]
    if not names:
        raise ValueError("no format given")
    formats = []
    for name in names:
        fmt = normalize_format(name)
        if fmt not in formats:
            formats.append(fmt)
    return formats


def export_all(dataset, formats: list[str], directory, header: bool = True) -> list[Path]:
    """Render `dataset` in every format and write the files into `directory`.

    Every document is rendered before anything is written, so a format that cannot be rendered
    (markdown without a header) leaves the directory untouched. Returns the paths written.
    """
    documents = []
    for fmt in formats:
        fmt = normalize_format(fmt)
        documents.append((Path(directory) / output_filename(dataset.title, fmt), render(dataset, fmt, header)))
    for path, text in documents:
        with open(path, "w", encoding="utf-8", newline="") as handle:
            handle.write(text)
    return [path for path, _ in documents]
