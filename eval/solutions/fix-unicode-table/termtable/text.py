"""Truncating, aligning and wrapping single-line text to a width in columns."""

from __future__ import annotations

from .width import display_width, graphemes, normalize

ELLIPSIS = "\u2026"
ALIGNMENTS = ("left", "right", "center")


def _take(parts: list[str], limit: int) -> tuple[int, int]:
    """Longest prefix of graphemes ``parts`` at most ``limit`` wide: (count, width)."""
    used = 0
    count = 0
    for part in parts:
        width = display_width(part)
        if used + width > limit:
            break
        used += width
        count += 1
    return count, used


def truncate(text: str, width: int) -> str:
    """Fit ``text`` into ``width`` columns, ending with an ellipsis when it is cut."""
    if width < 1:
        raise ValueError(f"width must be >= 1, got {width}")
    text = normalize(text)
    if display_width(text) <= width:
        return text
    parts = graphemes(text)
    count, used = _take(parts, width - 1)
    return "".join(parts[:count]) + " " * (width - 1 - used) + ELLIPSIS


def align(text: str, width: int, how: str = "left") -> str:
    """Truncate ``text`` to ``width`` columns and pad it with spaces to exactly ``width``."""
    if how not in ALIGNMENTS:
        raise ValueError(f"unknown alignment {how!r}")
    text = truncate(text, width)
    padding = width - display_width(text)
    left = {"left": 0, "right": padding, "center": padding // 2}[how]
    return " " * left + text + " " * (padding - left)


def _hard_break(word: str, width: int) -> list[str]:
    """Split a word into chunks of at most ``width`` columns at grapheme boundaries."""
    parts = graphemes(word)
    chunks = []
    while parts:
        count, _ = _take(parts, width)
        chunks.append("".join(parts[:count]))
        parts = parts[count:]
    return chunks


def wrap(text: str, width: int) -> list[str]:
    """Greedy word wrap at spaces; words wider than ``width`` are hard-broken."""
    if width < 2:
        raise ValueError(f"width must be >= 2, got {width}")
    lines: list[str] = []
    current = ""
    current_width = 0
    for word in normalize(text).split(" "):
        if not word:
            continue
        word_width = display_width(word)
        if current and current_width + 1 + word_width <= width:
            current += " " + word
            current_width += 1 + word_width
            continue
        if current:
            lines.append(current)
        chunks = [word] if word_width <= width else _hard_break(word, width)
        lines.extend(chunks[:-1])
        current = chunks[-1]
        current_width = display_width(current)
    if current:
        lines.append(current)
    return lines
