"""Clickstream analytics: sessions, per-session statistics and a summary report.

See README.md for the contract.
"""

from .events import Event, parse_events
from .report import Summary, build_report, render, summarize
from .sessions import Session, sessionize
from .stats import SessionStats, session_stats

__all__ = [
    "Event", "Session", "SessionStats", "Summary",
    "build_report", "parse_events", "render", "session_stats", "sessionize", "summarize",
]
