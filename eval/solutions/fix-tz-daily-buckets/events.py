"""Group event timestamps into local calendar days."""

from collections import Counter
from datetime import date, datetime, timedelta, timezone


def parse_timestamp(ts: str) -> datetime:
    """Parse an ISO-8601 timestamp with a `Z` or `+HH:MM` offset."""
    if ts.endswith(("Z", "z")):
        ts = ts[:-1] + "+00:00"  # Python 3.10's fromisoformat rejects "Z"
    parsed = datetime.fromisoformat(ts)
    if parsed.tzinfo is None:
        raise ValueError(f"timestamp has no UTC offset: {ts!r}")
    return parsed


def local_day(ts: str, utc_offset_minutes: int) -> date:
    """Calendar day of `ts` in a fixed UTC offset."""
    zone = timezone(timedelta(minutes=utc_offset_minutes))
    return parse_timestamp(ts).astimezone(zone).date()


def daily_counts(timestamps, utc_offset_minutes: int) -> dict[str, int]:
    """Count events per local day, keys in ascending order."""
    counts = Counter(local_day(ts, utc_offset_minutes).isoformat() for ts in timestamps)
    return dict(sorted(counts.items()))
