import unittest

from intervals import merge_intervals, total_covered


class MergeIntervalsTest(unittest.TestCase):
    def test_empty_input(self):
        self.assertEqual(merge_intervals([]), [])

    def test_disjoint_intervals_are_kept(self):
        self.assertEqual(merge_intervals([(1, 2), (4, 5)]), [(1, 2), (4, 5)])

    def test_overlapping_intervals_merge(self):
        self.assertEqual(merge_intervals([(1, 3), (2, 5)]), [(1, 5)])

    def test_touching_intervals_merge(self):
        self.assertEqual(merge_intervals([(1, 2), (2, 3)]), [(1, 3)])

    def test_nested_interval_keeps_outer_end(self):
        self.assertEqual(merge_intervals([(1, 10), (2, 3)]), [(1, 10)])

    def test_unsorted_input_is_sorted(self):
        self.assertEqual(merge_intervals([(7, 8), (1, 2), (4, 5)]), [(1, 2), (4, 5), (7, 8)])

    def test_zero_length_interval_is_valid(self):
        self.assertEqual(merge_intervals([(3, 3)]), [(3, 3)])

    def test_floats(self):
        self.assertEqual(merge_intervals([(0.5, 1.5), (1.5, 2.25)]), [(0.5, 2.25)])

    def test_returns_tuples(self):
        result = merge_intervals([[1, 2], [5, 6]])
        self.assertTrue(all(type(pair) is tuple for pair in result))

    def test_accepts_any_iterable(self):
        self.assertEqual(merge_intervals(iter([(2, 3), (1, 2)])), [(1, 3)])

    def test_input_is_not_modified(self):
        data = [[5, 6], [1, 4], [2, 3]]
        merge_intervals(data)
        self.assertEqual(data, [[5, 6], [1, 4], [2, 3]])

    def test_reversed_pair_raises(self):
        with self.assertRaises(ValueError):
            merge_intervals([(1, 2), (5, 4)])


class TotalCoveredTest(unittest.TestCase):
    def test_empty_input(self):
        self.assertEqual(total_covered([]), 0)

    def test_overlaps_count_once(self):
        self.assertEqual(total_covered([(0, 2), (1, 3), (10, 11)]), 4)

    def test_reversed_pair_raises(self):
        with self.assertRaises(ValueError):
            total_covered([(3, 1)])


if __name__ == "__main__":
    unittest.main()
