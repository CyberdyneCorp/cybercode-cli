import unittest

from stats import format_report, moving_average, windows
from stats.report import format_report as report_format_report
from stats.window import moving_average as window_moving_average


class HiddenWindowTest(unittest.TestCase):
    def test_visible_cases(self):
        self.assertEqual(windows([4, 5], 1), [[4], [5]])
        self.assertEqual(moving_average([3, 6, 9, 12], 3), [6.0, 9.0])

    def test_window_count_and_order(self):
        values = list(range(10))
        for size in range(1, 11):
            with self.subTest(size=size):
                got = [list(w) for w in windows(values, size)]
                self.assertEqual(got, [values[i : i + size] for i in range(11 - size)])

    def test_window_equal_to_length(self):
        self.assertEqual([list(w) for w in windows([1, 2, 3], 3)], [[1, 2, 3]])
        self.assertEqual(moving_average([2, 4], 2), [3.0])

    def test_tuple_input(self):
        self.assertEqual([list(w) for w in windows((1, 2, 3), 2)], [[1, 2], [2, 3]])

    def test_averages(self):
        got = moving_average([1, 2, 3, 4, 5, 6], 4)
        self.assertEqual(len(got), 3)
        for actual, expected in zip(got, [2.5, 3.5, 4.5]):
            self.assertAlmostEqual(actual, expected)

    def test_invalid_sizes_raise(self):
        for values, size in [([1, 2, 3], 4), ([1, 2, 3], 0), ([1, 2, 3], -1), ([], 1)]:
            with self.subTest(values=values, size=size):
                with self.assertRaises(ValueError):
                    windows(values, size)
                with self.assertRaises(ValueError):
                    moving_average(values, size)

    def test_report(self):
        self.assertEqual(format_report([1, 2, 3, 4], 2), ["day 2: 1.50", "day 3: 2.50", "day 4: 3.50"])
        self.assertEqual(format_report([10, 20, 30], 3), ["day 3: 20.00"])
        self.assertEqual(format_report([1, 2], 1), ["day 1: 1.00", "day 2: 2.00"])
        self.assertEqual(report_format_report([5, 7], 2), ["day 2: 6.00"])
        self.assertEqual(window_moving_average([5, 7], 1), [5.0, 7.0])

    def test_report_rejects_large_window(self):
        with self.assertRaises(ValueError):
            format_report([1, 2], 5)


if __name__ == "__main__":
    unittest.main()
