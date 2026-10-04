"""Display width of text in terminal columns.

Everything here works on *normalized* text: NFC, with tabs expanded to the next multiple of
TAB_SIZE columns (see README.md, sections 1-3).
"""

from __future__ import annotations

import unicodedata

TAB_SIZE = 4

# Invisible characters that take no column: zero width space, zero width joiner.
ZERO_WIDTH_CHARS = frozenset("\u200b\u200d")


def char_width(ch: str) -> int:
    """Columns taken by one character (not a tab): 0, 1 or 2."""
    if unicodedata.combining(ch) or ch in ZERO_WIDTH_CHARS:
        return 0
    if unicodedata.east_asian_width(ch) == "W":
        return 2
    return 1


def normalize(text: str) -> str:
    """NFC-normalize ``text`` and expand its tabs."""
    return unicodedata.normalize("NFC", text.expandtabs(TAB_SIZE))


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
