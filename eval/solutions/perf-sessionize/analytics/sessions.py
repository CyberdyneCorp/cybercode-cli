"""Group each user's events into sessions."""

from dataclasses import dataclass
from operator import attrgetter
from typing import Iterable

from .events import Event


@dataclass(frozen=True)
class Session:
    """A run of one user's events with no gap longer than the session gap."""

    session_id: str  # f"{user_id}#{n}", n = 1, 2, ... in time order per user
    user_id: str
    start: int  # timestamp of the first event
    end: int  # timestamp of the last event
    events: tuple  # the session's events, in session order


def check_gap(gap_seconds) -> None:
    if not isinstance(gap_seconds, int) or isinstance(gap_seconds, bool) or gap_seconds < 0:
        raise ValueError(f"gap_seconds must be a non-negative int, got {gap_seconds!r}")


def sessionize(events: Iterable[Event], gap_seconds: int) -> list[Session]:
    """Split events into per-user sessions; see README.md for the exact rules."""
    check_gap(gap_seconds)
    timelines: dict[str, list[Event]] = {}
    for event in events:
        timelines.setdefault(event.user_id, []).append(event)
    sessions = []
    for user, timeline in timelines.items():
        # list.sort is stable, so equal timestamps keep their input order.
        timeline.sort(key=attrgetter("timestamp"))
        sessions.extend(_split(user, timeline, gap_seconds))
    sessions.sort(key=session_order)
    return sessions


def _split(user: str, timeline: list[Event], gap_seconds: int) -> list[Session]:
    """Cut one user's time-ordered events wherever the gap exceeds gap_seconds."""
    groups = []
    for event in timeline:
        if groups and event.timestamp - groups[-1][-1].timestamp <= gap_seconds:
            groups[-1].append(event)
        else:
            groups.append([event])
    return [
        Session(f"{user}#{number}", user, group[0].timestamp, group[-1].timestamp, tuple(group))
        for number, group in enumerate(groups, start=1)
    ]


def session_order(session: Session) -> tuple:
    """Sessions are listed by start time, then user_id, then session number."""
    return (session.start, session.user_id, int(session.session_id.rsplit("#", 1)[1]))
