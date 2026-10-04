"""The append-only event log, one JSON object per line in `events.jsonl`."""

from __future__ import annotations

import os
from pathlib import Path

from .clock import Clock, stamp, system_clock
from .events import Event, PendingEvent, decode, encode

LOG_FILENAME = "events.jsonl"


class EventStore:
    def __init__(self, directory: str | os.PathLike, clock: Clock = system_clock):
        self._directory = Path(directory)
        self._directory.mkdir(parents=True, exist_ok=True)
        self._path = self._directory / LOG_FILENAME
        self._clock = clock
        self._last_seq = max((event.seq for event in self._load()), default=0)

    @property
    def path(self) -> Path:
        return self._path

    @property
    def last_seq(self) -> int:
        """Sequence number of the last event in the log (0 when empty)."""
        return self._last_seq

    @property
    def next_seq(self) -> int:
        """Sequence number the next appended event will get."""
        return self._last_seq + 1

    def append(self, pending: PendingEvent) -> Event:
        """Stamp `pending` with the next sequence number and the clock, and persist it."""
        event = Event(
            seq=self.next_seq,
            type=pending.type,
            wallet_id=pending.wallet_id,
            recorded_at=stamp(self._clock),
            data=dict(pending.data),
        )
        with open(self._path, "a", encoding="utf-8") as fh:
            fh.write(encode(event) + "\n")
            fh.flush()
            os.fsync(fh.fileno())
        self._last_seq = event.seq
        return event

    def read(self, after_seq: int = 0) -> list[Event]:
        """Return the events with a sequence number greater than `after_seq`."""
        events = [event for event in self._load() if event.seq > after_seq]
        # Several API hosts append to the same log, so lines are not necessarily written
        # in the order things happened; hand events out chronologically.
        events.sort(key=lambda event: event.recorded_at)
        return events

    def _load(self) -> list[Event]:
        if not self._path.exists():
            return []
        with open(self._path, encoding="utf-8") as fh:
            return [decode(line) for line in fh if line.strip()]
