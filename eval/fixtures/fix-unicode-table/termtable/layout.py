"""Column width allocation for tables that must fit a maximum width."""

from __future__ import annotations

SEPARATOR = " | "
MIN_COLUMN_WIDTH = 2


def table_width(widths: list[int]) -> int:
    """Total width of a table with these column widths, separators included."""
    return sum(widths) + len(SEPARATOR) * (len(widths) - 1)


def allocate(natural: list[int], max_width: int | None) -> list[int]:
    """Narrow the widest columns until the table fits ``max_width``."""
    widths = list(natural)
    if max_width is None:
        return widths
    excess = table_width(widths) - max_width
    while excess > 0:
        widest = widths.index(max(widths))
        if widths[widest] <= MIN_COLUMN_WIDTH:
            raise ValueError(f"table cannot fit in {max_width} columns")
        widths[widest] -= 1
        excess -= 1
    return widths
