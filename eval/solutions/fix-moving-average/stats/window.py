"""Sliding windows over a sequence of numbers."""


def windows(values, size):
    """Every contiguous window of `size` items, in order."""
    if size < 1 or size > len(values):
        raise ValueError(f"window size must be between 1 and {len(values)}, got {size}")
    return [list(values[i : i + size]) for i in range(len(values) - size + 1)]


def moving_average(values, size):
    """Mean of each window, in order."""
    return [sum(window) / size for window in windows(values, size)]
