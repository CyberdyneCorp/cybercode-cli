"""Render rows of text cells as an aligned plain-text table."""

from __future__ import annotations

from typing import Sequence

from .layout import SEPARATOR, allocate
from .text import ALIGNMENTS, align as align_text, wrap

OVERFLOW_MODES = ("truncate", "wrap")


def _check_shape(rows: list[list[str]], headers: list[str] | None) -> int:
    lines = ([headers] if headers is not None else []) + rows
    columns = len(lines[0])
    if columns == 0 or any(len(line) != columns for line in lines):
        raise ValueError("every row and the headers must have the same number of cells (at least one)")
    return columns


def _cell_lines(cell: str, width: int, overflow: str) -> list[str]:
    if overflow == "wrap" and len(cell) > width:
        return wrap(cell, width) or [""]
    return [cell]


def _render_row(cells: list[str], widths: list[int], aligns: list[str], overflow: str) -> list[str]:
    columns = [_cell_lines(cell, width, overflow) for cell, width in zip(cells, widths)]
    height = max(len(lines) for lines in columns)
    out = []
    for index in range(height):
        parts = []
        for lines, width, how in zip(columns, widths, aligns):
            line = lines[index] if index < len(lines) else ""
            parts.append(align_text(line, width, how))
        out.append(SEPARATOR.join(parts))
    return out


def render_table(
    rows: Sequence[Sequence[str]],
    headers: Sequence[str] | None = None,
    *,
    max_width: int | None = None,
    align: Sequence[str] | None = None,
    overflow: str = "truncate",
) -> str:
    """Lay out ``rows`` (and optional ``headers``) as described in README.md section 8."""
    rows = [list(row) for row in rows]
    headers = list(headers) if headers is not None else None
    if not rows and headers is None:
        return ""
    columns = _check_shape(rows, headers)
    aligns = list(align) if align is not None else ["left"] * columns
    if len(aligns) != columns or any(how not in ALIGNMENTS for how in aligns):
        raise ValueError(f"align must list one of {ALIGNMENTS} per column")
    if overflow not in OVERFLOW_MODES:
        raise ValueError(f"overflow must be one of {OVERFLOW_MODES}")

    every = ([headers] if headers is not None else []) + rows
    natural = [max(1, max(len(line[c]) for line in every)) for c in range(columns)]
    widths = allocate(natural, max_width)

    lines = []
    if headers is not None:
        lines += _render_row(headers, widths, aligns, overflow)
        lines.append("-+-".join("-" * width for width in widths))
    for row in rows:
        lines += _render_row(row, widths, aligns, overflow)
    return "\n".join(lines)
