"""Time build_report on the two large workloads stated in the task prompt.

Usage (from a directory containing the agent's `analytics` package):
    python3 perf_check.py WORKLOAD LIMIT_SECONDS
Exits 0 when the report matches the reference implementation's report and the call took at
most LIMIT_SECONDS; otherwise prints the reason and exits 1.
"""

import hashlib
import random
import sys
import time

import analytics
import reference_analytics


def workload_many_users(event_cls):
    """300,000 events from 50,000 users over 2,000 pages, in random order, within 30 days."""
    rng = random.Random(20240601)
    users = [f"user{n:05d}" for n in range(50_000)]
    pages = [f"/page/{n}" for n in range(2_000)]
    events = []
    for _ in range(300_000):
        events.append((rng.choice(users), rng.randrange(0, 30 * 86_400), rng.choice(pages)))
    return [event_cls(*fields) for fields in events]


def workload_few_users(event_cls):
    """300,000 events from 10 users over 5,000 pages, mostly 0-20 seconds apart per user."""
    rng = random.Random(20240602)
    pages = [f"/page/{n}" for n in range(5_000)]
    events = []
    for user in range(10):
        clock = 0
        for _ in range(30_000):
            clock += rng.randrange(0, 21) if rng.random() < 0.999 else 4_000
            events.append((f"heavy{user}", clock, rng.choice(pages)))
    rng.shuffle(events)
    return [event_cls(*fields) for fields in events]


WORKLOADS = {"many-users": workload_many_users, "few-users": workload_few_users}


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def main() -> int:
    name, limit = sys.argv[1], float(sys.argv[2])
    events = WORKLOADS[name](analytics.Event)
    started = time.perf_counter()
    report = analytics.build_report(events, 1800, 10)
    elapsed = time.perf_counter() - started
    expected = reference_analytics.build_report(WORKLOADS[name](reference_analytics.Event), 1800, 10)
    if report != expected:
        print(f"{name}: report differs from the reference")
        return 1
    if elapsed > limit:
        print(f"{name}: build_report took {elapsed:.2f}s, limit {limit:.1f}s")
        return 1
    print(f"{name}: build_report took {elapsed:.2f}s (limit {limit:.1f}s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
