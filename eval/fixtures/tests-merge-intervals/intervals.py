"""Merge numeric intervals. See README.md for the contract."""

from collections.abc import Iterable

Interval = tuple[float, float]


def merge_intervals(intervals: Iterable[tuple[float, float]]) -> list[Interval]:
    """Return the sorted union of `intervals`, merging overlapping and touching pairs."""
    pairs = [(start, end) for start, end in intervals]
    for start, end in pairs:
        if start > end:
            raise ValueError(f"interval start {start} is after end {end}")
    merged: list[Interval] = []
    for start, end in sorted(pairs):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def total_covered(intervals: Iterable[tuple[float, float]]) -> float:
    """Total length covered by `intervals`, counting overlaps once."""
    return sum(end - start for start, end in merge_intervals(intervals))
