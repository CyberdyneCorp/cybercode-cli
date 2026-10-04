"""Group event timestamps into local calendar days."""

from collections import Counter
from datetime import date, datetime, timedelta


def parse_timestamp(ts: str) -> datetime:
    """Parse an ISO-8601 timestamp with a `Z` or `+HH:MM` offset."""
    return datetime.fromisoformat(ts[:19])


def local_day(ts: str, utc_offset_minutes: int) -> date:
    """Calendar day of `ts` in a fixed UTC offset."""
    return (parse_timestamp(ts) + timedelta(minutes=utc_offset_minutes)).date()


def daily_counts(timestamps, utc_offset_minutes: int) -> dict[str, int]:
    """Count events per local day, keys in ascending order."""
    counts = Counter(local_day(ts, utc_offset_minutes).isoformat() for ts in timestamps)
    return dict(sorted(counts.items()))
