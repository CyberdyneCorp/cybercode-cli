"""Order totals."""

from . import catalog, discounts, money


def checkout(order: dict[str, int]) -> str:
    """Formatted total of an order mapping sku to quantity."""
    total = 0
    for sku, quantity in order.items():
        unit = money.to_cents(catalog.price_of(sku))
        total += discounts.apply_bulk_discount(unit * quantity, quantity)
    return money.format_cents(total)
