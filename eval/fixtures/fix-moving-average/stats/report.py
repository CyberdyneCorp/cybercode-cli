"""Text report of a smoothed daily series."""

from .window import moving_average


def format_report(daily, size):
    """One `day <D>: <avg>` line per window; D is the 1-based last day of the window."""
    averages = moving_average(daily, size)
    return [f"day {i + size - 1}: {avg:.2f}" for i, avg in enumerate(averages)]
