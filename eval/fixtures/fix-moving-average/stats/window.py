"""Sliding windows over a sequence of numbers."""


def windows(values, size):
    """Every contiguous window of `size` items, in order."""
    if size > len(values):
        return []
    return [list(values[i : i + size]) for i in range(len(values) - size)]


def moving_average(values, size):
    """Mean of each window, in order."""
    return [sum(window) / size for window in windows(values, size)]
