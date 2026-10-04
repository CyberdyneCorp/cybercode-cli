"""Mid-period plan changes.

When a subscription changes plan on day ``on`` inside a billing period
``[start, end)``, the customer is credited for the unused part of the old plan and
charged for the rest of the period on the new plan, both in proportion to the days
left in the actual billing period.
"""
from __future__ import annotations

from datetime import date
from decimal import Decimal

from .money import round_money
from .periods import day_count


def prorate(price: Decimal, currency: str, days_used_for: int, days_in_period: int) -> Decimal:
    """``price * days_used_for / days_in_period``, rounded once to the minor unit."""
    if days_in_period <= 0:
        raise ValueError("empty billing period")
    return round_money(price * days_used_for / days_in_period, currency)


def plan_change_amounts(old_price: Decimal, new_price: Decimal, currency: str,
                        start: date, end: date, on: date) -> tuple[Decimal, Decimal]:
    """Return ``(credit, charge)`` for a change on ``on`` within ``[start, end)``.

    ``credit`` is negative (or zero), ``charge`` positive (or zero).
    """
    if not start <= on < end:
        raise ValueError(f"{on} is outside the period {start}..{end}")
    total = day_count(start, end)
    remaining = day_count(on, end)
    credit = -prorate(old_price, currency, remaining, total)
    charge = prorate(new_price, currency, remaining, total)
    return credit, charge
