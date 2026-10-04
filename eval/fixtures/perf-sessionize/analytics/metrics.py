"""Small statistics helpers with exactly specified rounding."""

from decimal import ROUND_HALF_UP, Decimal


def lower_median(values: list[int]) -> int | None:
    """The middle value; for an even count the lower of the two middle values. None if empty."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[(len(ordered) - 1) // 2]


def nearest_rank(values: list[int], percent: int) -> int | None:
    """Nearest-rank percentile: the ceil(percent/100 * n)-th smallest value (1-based)."""
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, -(-percent * len(ordered) // 100))
    return ordered[rank - 1]


def ratio(numerator: int, denominator: int, scale: int = 1) -> str:
    """numerator * scale / denominator with two decimals, halves rounded up; "n/a" if denominator is 0."""
    if denominator == 0:
        return "n/a"
    value = Decimal(numerator * scale) / Decimal(denominator)
    return str(value.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP))


def count_ranking(keys: list[str]) -> list[tuple[str, int]]:
    """(key, occurrences) for every distinct key, most frequent first, ties by key ascending."""
    counts = []
    for key in keys:
        for entry in counts:
            if entry[0] == key:
                entry[1] += 1
                break
        else:
            counts.append([key, 1])
    counts.sort(key=lambda entry: (-entry[1], entry[0]))
    return [(key, count) for key, count in counts]
