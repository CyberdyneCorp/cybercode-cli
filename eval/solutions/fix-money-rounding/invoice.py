"""Invoice arithmetic. See README.md for the money rules."""

from decimal import ROUND_HALF_UP, Decimal

CENT = Decimal("0.01")


def to_cents(amount) -> Decimal:
    """Round a decimal amount to whole cents, half up."""
    return Decimal(str(amount)).quantize(CENT, rounding=ROUND_HALF_UP)


def line_total(unit_price, quantity):
    return str(to_cents(Decimal(unit_price) * quantity))


def invoice_total(lines, tax_rate):
    subtotal = sum((Decimal(line_total(price, quantity)) for price, quantity in lines), Decimal("0.00"))
    tax = to_cents(subtotal * Decimal(tax_rate))
    return {"subtotal": str(to_cents(subtotal)), "tax": str(tax), "total": str(to_cents(subtotal + tax))}


def split_amount(total, n):
    if n < 1:
        raise ValueError(f"cannot split into {n} shares")
    cents = int(to_cents(total) * 100)
    base, extra = divmod(cents, n)
    shares = [base + 1] * extra + [base] * (n - extra)
    return [str((Decimal(share) / 100).quantize(CENT)) for share in shares]
