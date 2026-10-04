import unittest
from datetime import date

from events import daily_counts, local_day


class EventsTest(unittest.TestCase):
    def test_utc_day(self):
        self.assertEqual(local_day("2024-03-10T12:00:00Z", 0), date(2024, 3, 10))

    def test_offset_timestamp_in_utc(self):
        # 23:30 at UTC-05:00 is 04:30 the next day in UTC.
        self.assertEqual(daily_counts(["2024-03-10T23:30:00-05:00"], 0), {"2024-03-11": 1})


if __name__ == "__main__":
    unittest.main()
