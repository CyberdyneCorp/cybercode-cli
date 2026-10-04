"""Calendar helpers for anniversary billing.

A subscription is billed in periods anchored on its start date (the *anchor*).
Period ``k`` starts ``k * interval_months`` months after the anchor, with the day of
month clamped to the end of shorter months, and ends (exclusive) where period
``k + 1`` starts. Every period is computed from the anchor, so a Jan 31 anchor gives
Jan 31, Feb 29 (leap year) or Feb 28, Mar 31, Apr 30, ...
"""
from __future__ import annotations

import calendar
from datetime import date


def days_in_month(year: int, month: int) -> int:
    return calendar.monthrange(year, month)[1]


def add_months(day: date, months: int) -> date:
    """Shift ``day`` by whole months, clamping the day of month to the target month."""
    index = day.year * 12 + (day.month - 1) + months
    year, month0 = divmod(index, 12)
    month = month0 + 1
    return date(year, month, min(day.day, days_in_month(year, month)))


def period_bounds(anchor: date, interval_months: int, index: int) -> tuple[date, date]:
    """Return ``(start, end)`` of period ``index`` (0-based); ``end`` is exclusive."""
    if index < 0:
        raise ValueError("period index must be >= 0")
    if interval_months < 1:
        raise ValueError("interval_months must be >= 1")
    start = add_months(anchor, index * interval_months)
    end = add_months(anchor, (index + 1) * interval_months)
    return start, end


def period_index(anchor: date, interval_months: int, on: date) -> int:
    """Index of the billing period that contains ``on``."""
    if on < anchor:
        raise ValueError(f"{on} is before the subscription start {anchor}")
    months = (on.year - anchor.year) * 12 + (on.month - anchor.month)
    index = max(0, months // interval_months - 1)
    while True:
        start, end = period_bounds(anchor, interval_months, index)
        if start <= on < end:
            return index
        index += 1


def day_count(start: date, end: date) -> int:
    """Number of days in ``[start, end)``."""
    return (end - start).days
