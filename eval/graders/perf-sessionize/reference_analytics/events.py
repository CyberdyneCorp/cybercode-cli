"""Page-view events and their CSV representation."""

import csv
import re
from dataclasses import dataclass
from typing import Iterable

HEADER = ["user_id", "timestamp", "page"]
_TIMESTAMP = re.compile(r"-?[0-9]+")


@dataclass(frozen=True)
class Event:
    """One page view: who, when (integer seconds) and which page."""

    user_id: str
    timestamp: int
    page: str


def parse_events(lines: Iterable[str]) -> list[Event]:
    """Parse CSV text lines (with a `user_id,timestamp,page` header) into events.

    Raises ValueError with a 1-based line number for malformed input.
    """
    rows = csv.reader(lines)
    header = next(rows, None)
    if header != HEADER:
        raise ValueError("line 1: expected header user_id,timestamp,page")
    events = []
    for number, row in enumerate(rows, start=2):
        if not row:
            continue
        events.append(_parse_row(row, number))
    return events


def _parse_row(row: list[str], number: int) -> Event:
    if len(row) != 3:
        raise ValueError(f"line {number}: expected 3 fields, got {len(row)}")
    user_id, timestamp, page = (field.strip() for field in row)
    if not user_id:
        raise ValueError(f"line {number}: empty user_id")
    if not _TIMESTAMP.fullmatch(timestamp):
        raise ValueError(f"line {number}: invalid timestamp {timestamp!r}")
    if not page:
        raise ValueError(f"line {number}: empty page")
    return Event(user_id, int(timestamp), page)
