# events

Daily activity counts for the usage dashboard (`events.py`, Python 3.10+, stdlib only).

Timestamps come from several services as ISO-8601 strings with an explicit UTC offset:
`YYYY-MM-DDTHH:MM:SS[.fff|.ffffff]` followed by `Z` or `+HH:MM` / `-HH:MM`.

Contract:

- `parse_timestamp(ts)` returns a timezone-aware `datetime` for the same instant. A timestamp
  without an offset raises `ValueError`. It must work on Python 3.10, whose
  `datetime.fromisoformat` does not accept the `Z` suffix.
- `local_day(ts, utc_offset_minutes)` returns the calendar `date` of that instant in a fixed
  UTC offset given in minutes (for example `-300` for UTC-05:00, `330` for UTC+05:30).
- `daily_counts(timestamps, utc_offset_minutes)` returns a dict mapping `"YYYY-MM-DD"` local days
  to event counts, with keys in ascending day order. Days without events are omitted.

The dashboard runs with the team's offset, but events near midnight land on the wrong day and
events from services that send `Z` or non-UTC offsets are miscounted. Never use the system clock or
local timezone. Run the tests with `python3 -m unittest`.
