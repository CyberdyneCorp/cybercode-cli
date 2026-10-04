import unittest

from stats import moving_average, windows


class WindowTest(unittest.TestCase):
    def test_window_of_one(self):
        self.assertEqual(windows([4, 5], 1), [[4], [5]])

    def test_three_point_average(self):
        self.assertEqual(moving_average([3, 6, 9, 12], 3), [6.0, 9.0])


if __name__ == "__main__":
    unittest.main()
