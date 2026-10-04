# intervals

`intervals.py` is a small, correct library with no tests yet. It needs a unittest suite in
`test_intervals.py`, run with `python3 -m unittest`.

## Contract

`merge_intervals(intervals)`

- `intervals` is any iterable of `(start, end)` pairs of numbers (ints or floats).
- Raises `ValueError` if any pair has `start > end`. Zero-length pairs (`start == end`) are valid.
- Returns a **new list of tuples**, sorted by start, in which overlapping intervals and intervals
  that merely touch (`(1, 2)` and `(2, 3)`) are merged into one. A merged interval spans from the
  smallest start to the largest end of the intervals it absorbed.
- Input may be in any order. The input object and the pairs in it are never modified.
- An empty input returns `[]`.

`total_covered(intervals)`

- Returns the total length covered by the intervals, counting overlapping stretches once
  (the sum of `end - start` over `merge_intervals(intervals)`). Raises `ValueError` like
  `merge_intervals`. Empty input returns `0`.
