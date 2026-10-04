"""Truncating, aligning and wrapping single-line text to a width in columns."""

from __future__ import annotations

import textwrap

from .width import display_width, normalize

ELLIPSIS = "\u2026"
ALIGNMENTS = ("left", "right", "center")


def truncate(text: str, width: int) -> str:
    """Fit ``text`` into ``width`` columns, ending with an ellipsis when it is cut."""
    if width < 1:
        raise ValueError(f"width must be >= 1, got {width}")
    if display_width(text) <= width:
        return text
    # Keep as much as fits in width - 1 columns, leaving room for the ellipsis.
    cut = width - 1
    while cut > 0 and display_width(text[:cut]) > width - 1:
        cut -= 1
    return text[:cut] + ELLIPSIS


def align(text: str, width: int, how: str = "left") -> str:
    """Truncate ``text`` to ``width`` columns and pad it with spaces to exactly ``width``."""
    if how not in ALIGNMENTS:
        raise ValueError(f"unknown alignment {how!r}")
    text = truncate(text, width)
    if how == "left":
        return text.ljust(width)
    if how == "right":
        return text.rjust(width)
    return text.center(width)


def wrap(text: str, width: int) -> list[str]:
    """Greedy word wrap at spaces; words wider than ``width`` are hard-broken."""
    if width < 2:
        raise ValueError(f"width must be >= 2, got {width}")
    return textwrap.wrap(normalize(text), width)
