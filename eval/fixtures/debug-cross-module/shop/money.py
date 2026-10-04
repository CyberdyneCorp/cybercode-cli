"""Integer-cent money helpers."""


def to_cents(amount) -> int:
    """Exact integer cents of a decimal string or number with at most two decimals."""
    return int(float(amount) * 100)


def format_cents(cents: int) -> str:
    """Render non-negative cents as `units.cc`."""
    return f"{cents // 100}.{cents % 100:02d}"
