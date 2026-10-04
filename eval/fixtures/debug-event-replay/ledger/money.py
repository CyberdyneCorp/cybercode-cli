"""Amounts are exact decimals with at most two fractional digits."""

from __future__ import annotations

from decimal import Decimal, InvalidOperation

from .errors import InvalidAmount

ZERO = Decimal("0")
MAX_FRACTION_DIGITS = 2


def parse_amount(value: str | Decimal) -> Decimal:
    """Return `value` as a Decimal, or raise InvalidAmount.

    Floats are rejected on purpose: by the time a float reaches us the amount is no longer
    exact, so callers must pass the original string.
    """
    if isinstance(value, bool) or not isinstance(value, (str, Decimal)):
        raise InvalidAmount(f"amount must be a decimal string, got {value!r}")
    try:
        amount = Decimal(value.strip()) if isinstance(value, str) else value
    except InvalidOperation:
        raise InvalidAmount(f"invalid amount: {value!r}") from None
    if not amount.is_finite():
        raise InvalidAmount(f"invalid amount: {value!r}")
    if amount <= ZERO:
        raise InvalidAmount(f"amount must be positive: {value!r}")
    if amount.as_tuple().exponent < -MAX_FRACTION_DIGITS:
        raise InvalidAmount(f"at most {MAX_FRACTION_DIGITS} fractional digits: {value!r}")
    return amount


def format_amount(amount: Decimal) -> str:
    """Render an amount for the event log, tools and logs (no exponent notation)."""
    return f"{amount:f}"
