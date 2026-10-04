# stats

Small helpers for smoothing daily metrics (`stats/`, Python 3.10+, stdlib only).

Contract:

- `windows(values, size)` returns a list of every contiguous window of `size` items from the
  sequence `values`, in order, each window as a list. A sequence of length `L` has `L - size + 1`
  windows. It raises `ValueError` when `size < 1` or `size > len(values)`.
- `moving_average(values, size)` returns the mean of each window as a float, in the same order,
  and raises `ValueError` in the same cases.
- `format_report(daily, size)` (in `stats/report.py`) returns one line per window,
  `day <D>: <avg>`, where `<D>` is the 1-based day number of the **last** day in the window and
  `<avg>` is the window mean with two decimals. `format_report([1, 2, 3, 4], 2)` returns
  `["day 2: 1.50", "day 3: 2.50", "day 4: 3.50"]`.

The dashboard always loses the most recent day, mislabels days, and silently shows nothing when
the window is too large. Run the tests with `python3 -m unittest`.
