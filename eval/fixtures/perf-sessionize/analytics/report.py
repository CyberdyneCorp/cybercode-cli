"""Summary report over all sessions."""

from dataclasses import dataclass
from typing import Iterable

from .events import Event
from .metrics import count_ranking, lower_median, nearest_rank, ratio
from .sessions import Session, sessionize
from .stats import session_stats, user_totals

TOP_PAGES = 5
# (label, lowest duration in the bucket); a bucket ends where the next one starts.
DURATION_BUCKETS = [("0s", 0), ("under 1m", 1), ("1m to under 10m", 60),
                    ("10m to under 30m", 600), ("30m or more", 1800)]


@dataclass(frozen=True)
class Summary:
    events: int
    users: int
    sessions: int
    bounce_rate: str  # percent of sessions that bounced, two decimals, or "n/a"
    pages_per_session: str  # two decimals, or "n/a"
    median_duration: int | None
    p90_duration: int | None
    top_users: list  # [(user_id, total_duration, session_count)], at most top_k
    top_pages: list  # [(page, views)], at most TOP_PAGES
    entry_pages: list  # [(page, sessions)], at most TOP_PAGES
    duration_buckets: list  # [(label, sessions)] for every bucket in DURATION_BUCKETS
    sessions_per_user: list  # [(session count, users)], by session count ascending


def summarize(sessions: list[Session], top_k: int = 10) -> Summary:
    """Aggregate statistics over sessions (as returned by sessionize)."""
    if not isinstance(top_k, int) or isinstance(top_k, bool) or top_k < 1:
        raise ValueError(f"top_k must be a positive int, got {top_k!r}")
    stats = session_stats(sessions)
    durations = [s.duration for s in stats]
    page_views = sum(s.page_count for s in stats)
    totals = user_totals(stats)
    ranked = sorted(totals, key=lambda total: (-total[1], total[0]))
    return Summary(
        events=page_views,
        users=len(totals),
        sessions=len(stats),
        bounce_rate=ratio(sum(1 for s in stats if s.bounce), len(stats), scale=100),
        pages_per_session=ratio(page_views, len(stats)),
        median_duration=lower_median(durations),
        p90_duration=nearest_rank(durations, 90),
        top_users=ranked[:top_k],
        top_pages=count_ranking([e.page for session in sessions for e in session.events])[:TOP_PAGES],
        entry_pages=count_ranking([s.entry_page for s in stats])[:TOP_PAGES],
        duration_buckets=_bucket_counts(durations),
        sessions_per_user=_sessions_per_user(totals),
    )


def _bucket_counts(durations: list[int]) -> list[tuple[str, int]]:
    counts = []
    for index, (label, low) in enumerate(DURATION_BUCKETS):
        high = DURATION_BUCKETS[index + 1][1] if index + 1 < len(DURATION_BUCKETS) else None
        counts.append((label, sum(1 for d in durations if d >= low and (high is None or d < high))))
    return counts


def _sessions_per_user(totals: list[tuple[str, int, int]]) -> list[tuple[int, int]]:
    counts = sorted({count for _, _, count in totals})
    return [(count, sum(1 for total in totals if total[2] == count)) for count in counts]


def render(summary: Summary) -> str:
    """The report as text; every line ends with a newline."""
    text = ""
    text += f"events: {summary.events}\n"
    text += f"users: {summary.users}\n"
    text += f"sessions: {summary.sessions}\n"
    text += f"bounce rate: {_percent(summary.bounce_rate)}\n"
    text += f"pages per session: {summary.pages_per_session}\n"
    text += f"median session duration: {_seconds(summary.median_duration)}\n"
    text += f"p90 session duration: {_seconds(summary.p90_duration)}\n"
    text += "top users by total session duration:\n"
    for rank, (user, total, count) in enumerate(summary.top_users, start=1):
        noun = "session" if count == 1 else "sessions"
        text += f"  {rank}. {user} {total}s in {count} {noun}\n"
    text += "top pages:\n"
    for page, views in summary.top_pages:
        text += f"  {page} {views}\n"
    text += "entry pages:\n"
    for page, sessions in summary.entry_pages:
        text += f"  {page} {sessions}\n"
    text += "session durations:\n"
    for label, sessions in summary.duration_buckets:
        text += f"  {label}: {sessions}\n"
    text += "sessions per user:\n"
    for count, users in summary.sessions_per_user:
        text += f"  {count}: {users} {'user' if users == 1 else 'users'}\n"
    return text


def build_report(events: Iterable[Event], gap_seconds: int = 1800, top_k: int = 10) -> str:
    """Sessionize the events and render the summary report."""
    return render(summarize(sessionize(events, gap_seconds), top_k))


def _percent(value: str) -> str:
    return value if value == "n/a" else f"{value}%"


def _seconds(value: int | None) -> str:
    return "n/a" if value is None else f"{value}s"
