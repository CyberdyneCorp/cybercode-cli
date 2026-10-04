"""Display width of text in terminal columns.

Everything here works on *normalized* text: NFC first, then tabs expanded to the next
multiple of TAB_SIZE columns (see README.md, sections 1-3).
"""

from __future__ import annotations

import unicodedata

TAB_SIZE = 4

# Zero-width characters named explicitly by the contract (all of them are also category Cf).
ZERO_WIDTH_CHARS = frozenset("\u200b\u200d\ufeff")
ZERO_WIDTH_CATEGORIES = frozenset({"Mn", "Me", "Cf"})
WIDE_EAST_ASIAN_WIDTHS = frozenset({"W", "F"})


def char_width(ch: str) -> int:
    """Columns taken by one character (not a tab): 0, 1 or 2."""
    if ch in ZERO_WIDTH_CHARS or unicodedata.category(ch) in ZERO_WIDTH_CATEGORIES:
        return 0
    if unicodedata.east_asian_width(ch) in WIDE_EAST_ASIAN_WIDTHS:
        return 2
    return 1


def expand_tabs(text: str) -> str:
    """Replace each tab by 1-4 spaces, measuring columns with char_width."""
    if "\t" not in text:
        return text
    parts = []
    column = 0
    for ch in text:
        if ch == "\t":
            spaces = TAB_SIZE - column % TAB_SIZE
            parts.append(" " * spaces)
            column += spaces
        else:
            parts.append(ch)
            column += char_width(ch)
    return "".join(parts)


def normalize(text: str) -> str:
    """NFC-normalize ``text``, then expand its tabs."""
    return expand_tabs(unicodedata.normalize("NFC", text))


def display_width(text: str) -> int:
    """Columns taken by ``text`` once normalized."""
    return sum(char_width(ch) for ch in normalize(text))


def graphemes(text: str) -> list[str]:
    """Split normalized ``text`` into graphemes: a character plus its trailing zero-width ones."""
    result: list[str] = []
    for ch in normalize(text):
        if result and char_width(ch) == 0:
            result[-1] += ch
        else:
            result.append(ch)
    return result
