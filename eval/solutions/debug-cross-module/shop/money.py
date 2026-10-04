"""Integer-cent money helpers."""

from decimal import ROUND_HALF_UP, Decimal


def to_cents(amount) -> int:
    """Exact integer cents of a decimal string or number with at most two decimals."""
    # float(amount) * 100 truncates 19.99 to 1998; go through Decimal instead.
    cents = Decimal(str(amount)) * 100
    return int(cents.to_integral_value(rounding=ROUND_HALF_UP))


def format_cents(cents: int) -> str:
    """Render non-negative cents as `units.cc`."""
    return f"{cents // 100}.{cents % 100:02d}"
