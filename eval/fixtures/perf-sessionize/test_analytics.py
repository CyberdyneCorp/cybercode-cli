import unittest

from analytics import Event, build_report, session_stats, sessionize


class SessionizeTest(unittest.TestCase):
    def test_splits_on_gap(self):
        events = [Event("u", 0, "/a"), Event("u", 100, "/b"), Event("u", 1000, "/c")]
        sessions = sessionize(events, 500)
        self.assertEqual([s.session_id for s in sessions], ["u#1", "u#2"])
        self.assertEqual([len(s.events) for s in sessions], [2, 1])

    def test_unsorted_input(self):
        events = [Event("u", 50, "/b"), Event("u", 10, "/a")]
        (session,) = sessionize(events, 60)
        self.assertEqual([e.page for e in session.events], ["/a", "/b"])
        self.assertEqual((session.start, session.end), (10, 50))


class StatsTest(unittest.TestCase):
    def test_stats(self):
        events = [Event("u", 0, "/a"), Event("u", 30, "/a"), Event("v", 5, "/x")]
        stats = session_stats(sessionize(events, 60))
        self.assertEqual([(s.session_id, s.duration, s.page_count, s.distinct_pages, s.bounce) for s in stats],
                         [("u#1", 30, 2, 1, False), ("v#1", 0, 1, 1, True)])


class ReportTest(unittest.TestCase):
    def test_report_header(self):
        events = [Event("u", 0, "/a"), Event("u", 30, "/b"), Event("v", 5, "/a")]
        lines = build_report(events, 60, 5).splitlines()
        self.assertEqual(lines[:4], ["events: 3", "users: 2", "sessions: 2", "bounce rate: 50.00%"])


if __name__ == "__main__":
    unittest.main()
