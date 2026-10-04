import unittest
from datetime import date, datetime, timedelta, timezone

from events import daily_counts, local_day, parse_timestamp


class HiddenEventsTest(unittest.TestCase):
    def test_visible_cases(self):
        self.assertEqual(local_day("2024-03-10T12:00:00Z", 0), date(2024, 3, 10))
        self.assertEqual(daily_counts(["2024-03-10T23:30:00-05:00"], 0), {"2024-03-11": 1})

    def test_parse_returns_same_instant(self):
        cases = {
            "2024-03-10T12:00:00Z": datetime(2024, 3, 10, 12, tzinfo=timezone.utc),
            "2024-03-10T12:00:00.250Z": datetime(2024, 3, 10, 12, 0, 0, 250000, tzinfo=timezone.utc),
            "2024-03-10T23:30:00-05:00": datetime(2024, 3, 11, 4, 30, tzinfo=timezone.utc),
            "2024-03-11T05:15:00+05:30": datetime(2024, 3, 10, 23, 45, tzinfo=timezone.utc),
            "2024-03-10T08:00:00.123456+01:00": datetime(2024, 3, 10, 7, 0, 0, 123456, tzinfo=timezone.utc),
        }
        for ts, instant in cases.items():
            with self.subTest(ts=ts):
                parsed = parse_timestamp(ts)
                self.assertIsNotNone(parsed.tzinfo)
                self.assertEqual(parsed, instant)

    def test_naive_timestamp_rejected(self):
        with self.assertRaises(ValueError):
            parse_timestamp("2024-03-10T12:00:00")

    def test_local_day_across_midnight(self):
        cases = [
            ("2024-03-10T23:30:00-05:00", -300, date(2024, 3, 10)),
            ("2024-03-10T23:30:00-05:00", 0, date(2024, 3, 11)),
            ("2024-03-11T03:00:00Z", -300, date(2024, 3, 10)),
            ("2024-03-10T20:00:00Z", 330, date(2024, 3, 11)),
            ("2024-03-10T18:29:59Z", 330, date(2024, 3, 10)),
            ("2024-03-10T18:30:00Z", 330, date(2024, 3, 11)),
            ("2024-12-31T23:59:59-01:00", 60, date(2025, 1, 1)),
            ("2024-02-29T00:30:00+02:00", 0, date(2024, 2, 28)),
            ("2024-03-01T01:00:00+14:00", -600, date(2024, 2, 29)),
        ]
        for ts, offset, expected in cases:
            with self.subTest(ts=ts, offset=offset):
                self.assertEqual(local_day(ts, offset), expected)

    def test_daily_counts_mixed_offsets(self):
        events = [
            "2024-03-10T04:59:59Z",        # Mar 9, 23:59:59 at -05:00
            "2024-03-10T05:00:00Z",        # Mar 10, 00:00 at -05:00
            "2024-03-10T09:00:00+04:00",   # 05:00Z -> Mar 10
            "2024-03-10T23:30:00-05:00",   # Mar 10
            "2024-03-11T00:10:00-05:00",   # Mar 11
            "2024-03-09T12:00:00.500Z",    # Mar 9
        ]
        counts = daily_counts(events, -300)
        self.assertEqual(counts, {"2024-03-09": 2, "2024-03-10": 3, "2024-03-11": 1})
        self.assertEqual(list(counts), sorted(counts))

    def test_daily_counts_order_and_empty(self):
        events = ["2024-05-03T10:00:00Z", "2024-05-01T10:00:00Z", "2024-05-02T10:00:00Z", "2024-05-01T11:00:00Z"]
        counts = daily_counts(events, 0)
        self.assertEqual(list(counts.items()), [("2024-05-01", 2), ("2024-05-02", 1), ("2024-05-03", 1)])
        self.assertEqual(daily_counts([], 120), {})

    def test_accepts_generator(self):
        counts = daily_counts((ts for ts in ["2024-01-01T00:00:00Z"]), 0)
        self.assertEqual(counts, {"2024-01-01": 1})


if __name__ == "__main__":
    unittest.main()
