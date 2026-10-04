"""Renewal schedule helpers built on the anniversary periods in periods.py."""
from __future__ import annotations

from datetime import date

from .periods import period_bounds, period_index


def upcoming_periods(anchor: date, interval_months: int, count: int,
                     on: date | None = None) -> list[tuple[date, date]]:
    """``count`` consecutive periods starting with the one containing ``on``.

    Without ``on`` the schedule starts at the first period.
    """
    if count < 0:
        raise ValueError("count must be >= 0")
    first = 0 if on is None else period_index(anchor, interval_months, on)
    return [period_bounds(anchor, interval_months, first + k) for k in range(count)]


def next_renewal(anchor: date, interval_months: int, on: date) -> date:
    """The first period start strictly after ``on`` (the date the next invoice is issued)."""
    index = period_index(anchor, interval_months, on)
    return period_bounds(anchor, interval_months, index)[1]
