"""Money helpers: currency minor units, rounding and formatting.

Every amount in this package is a ``decimal.Decimal``. Binary floats are rejected
here so that they cannot leak into invoice arithmetic.
"""
from __future__ import annotations

from decimal import ROUND_HALF_UP, Decimal

ZERO = Decimal(0)

# ISO 4217 alpha code -> (numeric code, minor units).
CURRENCIES: dict[str, tuple[str, int]] = {
    "USD": ("840", 2),
    "EUR": ("978", 2),
    "GBP": ("826", 2),
    "CHF": ("756", 2),
    "JPY": ("392", 0),
    "KRW": ("410", 0),
    "BHD": ("048", 3),
    "KWD": ("414", 3),
}

# Payment providers report numeric codes; map them back to the alpha code.
_ALPHA_BY_NUMERIC = {numeric: alpha for alpha, (numeric, _) in CURRENCIES.items()}


def normalize_currency(code: str) -> str:
    """Return the ISO alpha code for an alpha (any case) or numeric currency code."""
    text = str(code).strip().upper()
    if text in CURRENCIES:
        return text
    if text in _ALPHA_BY_NUMERIC:
        return _ALPHA_BY_NUMERIC[text]
    raise ValueError(f"unknown currency: {code}")


def minor_units(currency: str) -> int:
    """Number of decimal places of the currency's minor unit (USD 2, JPY 0, BHD 3)."""
    return CURRENCIES[normalize_currency(currency)][1]


def _check_decimal(amount: Decimal) -> Decimal:
    if isinstance(amount, float):
        raise TypeError("money amounts must be Decimal, not float")
    if isinstance(amount, int) and not isinstance(amount, bool):
        return Decimal(amount)
    if not isinstance(amount, Decimal):
        raise TypeError(f"money amounts must be Decimal, got {type(amount).__name__}")
    return amount


def round_money(amount: Decimal, currency: str) -> Decimal:
    """Round to the currency's minor unit, ties away from zero (ROUND_HALF_UP)."""
    amount = _check_decimal(amount)
    exponent = Decimal(1).scaleb(-minor_units(currency))
    return amount.quantize(exponent, rounding=ROUND_HALF_UP)


def format_amount(amount: Decimal, currency: str) -> str:
    """Render an amount with exactly the currency's number of decimals.

    Negative amounts get a leading ``-``; zero is never rendered as ``-0``.
    """
    rounded = round_money(amount, currency)
    if rounded == 0:
        rounded = abs(rounded)
    return f"{rounded:f}"


def sum_money(amounts, currency: str) -> Decimal:
    """Sum already-rounded amounts; the result is expressed in the currency's minor units."""
    total = ZERO
    for amount in amounts:
        total += _check_decimal(amount)
    return round_money(total, currency)
