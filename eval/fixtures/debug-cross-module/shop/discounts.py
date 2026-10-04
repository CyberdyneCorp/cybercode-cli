"""Quantity discounts."""

BULK_QUANTITY = 10


def apply_bulk_discount(line_cents: int, quantity: int) -> int:
    """10% off a line of BULK_QUANTITY or more units, rounded half up to the cent."""
    if quantity < BULK_QUANTITY:
        return line_cents
    discount = (line_cents + 5) // 10
    return line_cents - discount
