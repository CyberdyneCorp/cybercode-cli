"""Hidden differential tests: the workspace's analytics package must behave exactly like the
original (frozen in original_analytics/) on many small adversarial inputs."""

import os
import random
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import analytics
import original_analytics as original

SEED = 1234
CASES = 400
USERS = ["alice", "bob", "Bob", "carol", "a#1", "a", "zed", "user10", "user9"]
PAGES = ["/", "/home", "/Home", "/cart", "/pay", "/a", "/b"]


def random_events(rng: random.Random):
    """Few users, few pages and a small time range, so ties and duplicates are common."""
    users = rng.sample(USERS, rng.randint(1, len(USERS)))
    span = rng.choice([0, 3, 10, 60, 4000])
    events = []
    for _ in range(rng.randint(0, 40)):
        if events and rng.random() < 0.15:
            events.append(rng.choice(events))  # an exact duplicate event
        else:
            events.append((rng.choice(users), rng.randint(-5, span), rng.choice(PAGES)))
    return events


def gap_choice(rng: random.Random) -> int:
    return rng.choice([0, 1, 2, 5, 10, 30, 1800])


def session_view(sessions):
    return [(s.session_id, s.user_id, s.start, s.end, type(s.events) is tuple,
             [(e.user_id, e.timestamp, e.page) for e in s.events]) for s in sessions]


def stats_view(stats):
    return [(s.session_id, s.user_id, s.start, s.duration, s.page_count, s.distinct_pages,
             s.bounce, s.entry_page, s.exit_page) for s in stats]


SUMMARY_FIELDS = ["events", "users", "sessions", "bounce_rate", "pages_per_session", "median_duration",
                  "p90_duration", "top_users", "top_pages", "entry_pages", "duration_buckets",
                  "sessions_per_user"]


def summary_view(summary):
    return {name: [tuple(item) for item in value] if isinstance(value, list) else value
            for name, value in ((name, getattr(summary, name)) for name in SUMMARY_FIELDS)}


class DifferentialTest(unittest.TestCase):
    def cases(self):
        rng = random.Random(SEED)
        for number in range(CASES):
            yield number, random_events(rng), gap_choice(rng), rng.choice([1, 2, 3, 10])

    def test_sessionize(self):
        for number, rows, gap, _ in self.cases():
            with self.subTest(case=number, gap=gap, events=rows):
                mine = analytics.sessionize([analytics.Event(*row) for row in rows], gap)
                theirs = original.sessionize([original.Event(*row) for row in rows], gap)
                self.assertEqual(session_view(mine), session_view(theirs))

    def test_sessionize_accepts_any_iterable(self):
        rows = [("u", 30, "/b"), ("u", 10, "/a"), ("v", 10, "/a"), ("u", 10, "/c")]
        mine = analytics.sessionize((analytics.Event(*row) for row in rows), 5)
        theirs = original.sessionize((original.Event(*row) for row in rows), 5)
        self.assertEqual(session_view(mine), session_view(theirs))

    def test_session_stats(self):
        for number, rows, gap, _ in self.cases():
            with self.subTest(case=number, gap=gap, events=rows):
                mine = analytics.session_stats(analytics.sessionize([analytics.Event(*row) for row in rows], gap))
                theirs = original.session_stats(original.sessionize([original.Event(*row) for row in rows], gap))
                self.assertEqual(stats_view(mine), stats_view(theirs))

    def test_summarize(self):
        for number, rows, gap, top_k in self.cases():
            with self.subTest(case=number, gap=gap, top_k=top_k, events=rows):
                mine = analytics.summarize(analytics.sessionize([analytics.Event(*row) for row in rows], gap), top_k)
                theirs = original.summarize(original.sessionize([original.Event(*row) for row in rows], gap), top_k)
                self.assertEqual(summary_view(mine), summary_view(theirs))

    def test_build_report(self):
        for number, rows, gap, top_k in self.cases():
            with self.subTest(case=number, gap=gap, top_k=top_k, events=rows):
                mine = analytics.build_report([analytics.Event(*row) for row in rows], gap, top_k)
                theirs = original.build_report([original.Event(*row) for row in rows], gap, top_k)
                self.assertEqual(mine, theirs)

    def test_report_statistics_on_wide_ranges(self):
        # Larger inputs with spread-out durations exercise the median, percentile and buckets.
        rng = random.Random(SEED + 1)
        for number in range(60):
            rows = [(f"u{rng.randrange(40)}", rng.randrange(0, 20_000), f"/p{rng.randrange(12)}")
                    for _ in range(rng.randint(1, 400))]
            gap = rng.choice([60, 600, 1800, 2400])
            with self.subTest(case=number, gap=gap):
                mine = analytics.build_report([analytics.Event(*row) for row in rows], gap, 5)
                theirs = original.build_report([original.Event(*row) for row in rows], gap, 5)
                self.assertEqual(mine, theirs)

    def test_invalid_arguments(self):
        event = [analytics.Event("u", 1, "/")]
        for gap in [-1, True, 1.5, "10", None]:
            with self.subTest(gap=gap), self.assertRaises(ValueError):
                analytics.sessionize(event, gap)
        for top_k in [0, -1, True, 2.0]:
            with self.subTest(top_k=top_k), self.assertRaises(ValueError):
                analytics.summarize(analytics.sessionize(event, 10), top_k)


CSV_INPUTS = {
    "basic.csv": "user_id,timestamp,page\nalice,100,/home\nbob,5,/a\nalice,1950,/cart\nalice,100,/pay\n\nbob,5, /b \n",
    "bad_header.csv": "user,timestamp,page\nalice,1,/\n",
    "bad_timestamp.csv": "user_id,timestamp,page\nalice,1,/\nbob,1.5,/x\n",
    "bad_fields.csv": "user_id,timestamp,page\nalice,1\n",
    "empty_user.csv": "user_id,timestamp,page\n ,1,/\n",
    "empty.csv": "user_id,timestamp,page\n",
}
CLI_ARGS = [[], ["--gap", "0"], ["--gap", "1849", "--top", "1"], ["--top", "0"], ["--gap", "-5"]]


class CliTest(unittest.TestCase):
    def run_cli(self, package: str, args: list[str], cwd: Path):
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        result = subprocess.run([sys.executable, "-m", package, *args], cwd=cwd, env=env,
                                capture_output=True, text=True, timeout=30)
        return result.returncode, result.stdout, result.stderr.replace(f"-m {package}", "-m analytics")

    def test_cli_matches_original(self):
        here = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory() as tmp:
            for name, text in CSV_INPUTS.items():
                (Path(tmp) / name).write_text(text)
            for name in [*CSV_INPUTS, "missing.csv"]:
                for args in CLI_ARGS:
                    argv = [str(Path(tmp) / name), *args]
                    with self.subTest(file=name, args=args):
                        self.assertEqual(self.run_cli("analytics", argv, here),
                                         self.run_cli("original_analytics", argv, here))


if __name__ == "__main__":
    unittest.main()
