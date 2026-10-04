"""Per-session statistics."""

from dataclasses import dataclass

from .sessions import Session


@dataclass(frozen=True)
class SessionStats:
    session_id: str
    user_id: str
    start: int
    duration: int  # end - start, in seconds
    page_count: int  # number of events (page views)
    distinct_pages: int  # number of different pages viewed
    bounce: bool  # exactly one page view
    entry_page: str  # page of the first event
    exit_page: str  # page of the last event


def session_stats(sessions: list[Session]) -> list[SessionStats]:
    """One SessionStats per session, in the same order."""
    result = []
    for session in sessions:
        result.append(SessionStats(
            session_id=session.session_id,
            user_id=session.user_id,
            start=session.start,
            duration=session.end - session.start,
            page_count=len(session.events),
            distinct_pages=len({event.page for event in session.events}),
            bounce=len(session.events) == 1,
            entry_page=session.events[0].page,
            exit_page=session.events[-1].page,
        ))
    return result


def user_totals(stats: list[SessionStats]) -> list[tuple[str, int, int]]:
    """(user_id, total duration, session count) per user, ordered by user_id."""
    durations: dict[str, int] = {}
    counts: dict[str, int] = {}
    for s in stats:
        durations[s.user_id] = durations.get(s.user_id, 0) + s.duration
        counts[s.user_id] = counts.get(s.user_id, 0) + 1
    return [(user, durations[user], counts[user]) for user in sorted(durations)]
