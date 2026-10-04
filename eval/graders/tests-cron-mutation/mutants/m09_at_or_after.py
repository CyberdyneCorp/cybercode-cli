# Mutant: next_fire may return the truncated `after` minute itself
"""Five-field cron expressions: matching and next fire time. See README.md for the contract."""

import calendar
import re
from dataclasses import dataclass
from datetime import date, datetime, timedelta

MONTH_NAMES = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"]
DAY_NAMES = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"]

# (name, minimum, maximum, names mapped to values)
FIELDS = [
    ("minute", 0, 59, {}),
    ("hour", 0, 23, {}),
    ("day of month", 1, 31, {}),
    ("month", 1, 12, {name: number for number, name in enumerate(MONTH_NAMES, start=1)}),
    ("day of week", 0, 7, {name: number for number, name in enumerate(DAY_NAMES)}),
]

MACROS = {
    "@yearly": "0 0 1 1 *",
    "@annually": "0 0 1 1 *",
    "@monthly": "0 0 1 * *",
    "@weekly": "0 0 * * 0",
    "@daily": "0 0 * * *",
    "@midnight": "0 0 * * *",
    "@hourly": "0 * * * *",
}

SEARCH_YEARS = 8

_ITEM = re.compile(r"(?P<base>\*|(?P<start>[0-9A-Za-z]+)(?:-(?P<end>[0-9A-Za-z]+))?)(?:/(?P<step>[0-9]+))?")


@dataclass(frozen=True)
class Schedule:
    minutes: frozenset
    hours: frozenset
    days: frozenset
    months: frozenset
    weekdays: frozenset  # 0 = Sunday ... 6 = Saturday
    days_restricted: bool
    weekdays_restricted: bool


def parse(expr: str) -> Schedule:
    """Parse an expression into a Schedule; raise ValueError if it is invalid."""
    expr = expr.strip()
    if expr.startswith("@"):
        if expr not in MACROS:
            raise ValueError(f"unknown macro: {expr!r}")
        expr = MACROS[expr]
    fields = expr.split()
    if len(fields) != len(FIELDS):
        raise ValueError(f"expected {len(FIELDS)} fields, got {len(fields)}: {expr!r}")
    minutes, hours, days, months, weekdays = (
        _parse_field(text, *spec) for text, spec in zip(fields, FIELDS)
    )
    return Schedule(
        minutes=minutes,
        hours=hours,
        days=days,
        months=months,
        weekdays=frozenset(day % 7 for day in weekdays),
        days_restricted=fields[2] != "*",
        weekdays_restricted=fields[4] != "*",
    )


def _parse_field(text: str, name: str, low: int, high: int, names: dict) -> frozenset:
    values = set()
    for item in text.split(","):
        match = _ITEM.fullmatch(item)
        if not match:
            raise ValueError(f"invalid {name} item: {item!r}")
        if match["base"] == "*":
            start, end = low, high
        else:
            start = _value(match["start"], name, low, high, names)
            end = _value(match["end"], name, low, high, names) if match["end"] else start
            if match["step"] and not match["end"]:
                end = high
        if start > end:
            raise ValueError(f"invalid {name} range: {item!r}")
        step = int(match["step"]) if match["step"] else 1
        if step < 1:
            raise ValueError(f"invalid {name} step: {item!r}")
        values.update(range(start, end + 1, step))
    return frozenset(values)


def _value(token: str, name: str, low: int, high: int, names: dict) -> int:
    if token.isdigit():
        number = int(token)
    elif token.upper() in names:
        number = names[token.upper()]
    else:
        raise ValueError(f"invalid {name} value: {token!r}")
    if not low <= number <= high:
        raise ValueError(f"{name} value out of range: {token!r}")
    return number


def _check_naive(moment: datetime) -> None:
    if moment.tzinfo is not None:
        raise ValueError("timezone-aware datetimes are not supported")


def _day_matches(schedule: Schedule, day: date) -> bool:
    dom = day.day in schedule.days
    dow = (day.weekday() + 1) % 7 in schedule.weekdays
    if schedule.days_restricted and schedule.weekdays_restricted:
        return dom or dow
    return dom and dow


def matches(expr: str, dt: datetime) -> bool:
    """Whether `expr` fires at the minute of `dt` (seconds and microseconds are ignored)."""
    schedule = parse(expr)
    _check_naive(dt)
    return (
        dt.minute in schedule.minutes
        and dt.hour in schedule.hours
        and dt.month in schedule.months
        and _day_matches(schedule, dt.date())
    )


def next_fire(expr: str, after: datetime) -> datetime:
    """The first minute strictly after `after` (truncated to the minute) that `expr` matches."""
    schedule = parse(expr)
    _check_naive(after)
    start = after.replace(second=0, microsecond=0)
    for year in range(start.year, after.year + SEARCH_YEARS + 1):
        for month in sorted(schedule.months):
            if (year, month) < (start.year, start.month):
                continue
            for day_number in range(1, calendar.monthrange(year, month)[1] + 1):
                day = date(year, month, day_number)
                if day < start.date() or not _day_matches(schedule, day):
                    continue
                found = _first_time_on(schedule, day, start)
                if found:
                    return found
    raise ValueError(f"{expr!r} does not fire within {SEARCH_YEARS} years after {after}")


def _first_time_on(schedule: Schedule, day: date, start: datetime) -> datetime | None:
    for hour in sorted(schedule.hours):
        if day == start.date() and hour < start.hour:
            continue
        for minute in sorted(schedule.minutes):
            if day == start.date() and hour == start.hour and minute < start.minute:
                continue
            return datetime(day.year, day.month, day.day, hour, minute)
    return None
