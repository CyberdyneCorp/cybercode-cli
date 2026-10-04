"""Clocks and event timestamps.

A clock is any zero-argument callable returning a timezone-aware datetime. The service
stamps every appended event with one reading of its clock.
"""

from __future__ import annotations

from datetime import datetime, timezone
from typing import Callable

Clock = Callable[[], datetime]


def system_clock() -> datetime:
    """The host's clock, in the host's local UTC offset."""
    return datetime.now().astimezone()


def stamp(clock: Clock) -> str:
    """Read `clock` once and return the reading as an ISO-8601 string with its offset."""
    now = clock()
    if now.tzinfo is None or now.utcoffset() is None:
        raise ValueError("clock must return timezone-aware datetimes")
    return now.isoformat()


def parse_stamp(text: str) -> datetime:
    """Parse a timestamp produced by `stamp`."""
    return datetime.fromisoformat(text)


def to_utc(text: str) -> datetime:
    return parse_stamp(text).astimezone(timezone.utc)
