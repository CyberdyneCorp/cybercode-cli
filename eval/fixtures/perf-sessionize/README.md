# analytics

Clickstream analytics (Python 3.10+, standard library only): group page-view events into
per-user sessions, compute per-session statistics and render a summary report.

```sh
python3 -m analytics events.csv [--gap SECONDS] [--top K]   # defaults: --gap 1800 --top 10
python3 -m unittest                                          # tests
```

The behavior below is the contract. All outputs (return values, report text, CLI output and
error messages) are exact.

## Events (`analytics/events.py`)

`Event(user_id: str, timestamp: int, page: str)` is a frozen dataclass: one page view;
`timestamp` is in integer seconds (may be negative). Events may arrive in any order, and the
same user may have several events with the same timestamp (even identical events).

`parse_events(lines)` reads CSV text lines with the header `user_id,timestamp,page`; blank
rows are skipped, fields are stripped of surrounding whitespace, and malformed input raises
`ValueError` with a 1-based line number (see the code for the exact messages).

## Sessions (`analytics/sessions.py`)

`sessionize(events, gap_seconds) -> list[Session]`, where `events` is any iterable of `Event`
and `gap_seconds` a non-negative `int` (anything else, including a `bool`, raises
`ValueError`).

1. Each user's events are put in *session order*: by timestamp ascending, and events with
   equal timestamps keep their relative order from the input (their position in `events`).
2. Walking a user's events in that order, an event starts a new session when its timestamp
   minus the previous event's timestamp is **strictly greater** than `gap_seconds`; otherwise
   it joins the current session (a gap exactly equal to `gap_seconds` does not split).
3. A user's sessions are numbered 1, 2, 3, ... in time order; `session_id` is
   `f"{user_id}#{n}"`.
4. `Session(session_id, user_id, start, end, events)` is a frozen dataclass: `start` and `end`
   are the timestamps of the session's first and last events and `events` is a tuple of its
   events in session order.
5. The returned list is ordered by `start`, then `user_id` (ascending string order), then
   session number.

## Statistics (`analytics/stats.py`)

`session_stats(sessions) -> list[SessionStats]` returns one frozen dataclass per session, in
the same order, with: `session_id`, `user_id`, `start`; `duration = end - start`;
`page_count` (number of events); `distinct_pages` (number of different `page` strings,
case-sensitive); `bounce` (`page_count == 1`); `entry_page` and `exit_page` (pages of the
first and last events in session order).

## Report (`analytics/report.py`)

`summarize(sessions, top_k=10) -> Summary` (`top_k` must be a positive `int`, otherwise
`ValueError`) computes, over the given sessions:

- `events`: total page views; `users`: distinct users; `sessions`: number of sessions.
- `bounce_rate`: `100 * bounced sessions / sessions` and `pages_per_session`:
  `events / sessions`, each as a string with exactly two decimals, computed in decimal
  arithmetic with halves rounded up (`"33.33"`, `"12.50"`); `"n/a"` when there are no
  sessions.
- `median_duration`: the **lower median** of the session durations (the middle value of the
  sorted durations; for an even count, the lower of the two middle values); `None` when there
  are no sessions.
- `p90_duration`: the 90th percentile of the durations by the **nearest-rank** method: the
  value at 1-based position `ceil(90 * n / 100)` of the sorted durations (computed exactly,
  without floating point); `None` when there are no sessions.
- `top_users`: `(user_id, total duration, session count)` for the `top_k` users with the
  largest total session duration; ties by `user_id` ascending.
- `top_pages`: `(page, views)` for the 5 most viewed pages (over all events of the sessions);
  `entry_pages`: `(page, sessions)` for the 5 most common entry pages; both with ties by page
  ascending.
- `duration_buckets`: `(label, sessions)` for each bucket, in this order and always all five:
  `0s` (duration 0), `under 1m` (1-59), `1m to under 10m` (60-599), `10m to under 30m`
  (600-1799), `30m or more` (1800 and above).
- `sessions_per_user`: `(n, users)` for every session count `n` that at least one user has,
  by `n` ascending, where `users` is the number of users with exactly `n` sessions.

`render(summary) -> str` formats the summary (see `render` for the exact text; every line ends
with `\n`), and `build_report(events, gap_seconds=1800, top_k=10)` is
`render(summarize(sessionize(events, gap_seconds), top_k))`. The CLI prints
`build_report(parse_events(file), --gap, --top)` to stdout, or `error: <message>` to stderr
with exit status 1 for an unreadable file or invalid CSV.
