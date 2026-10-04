"""Unit prices, as decimal strings so no precision is lost on the way in."""

PRICES = {
    "pen": "1.50",
    "notebook": "4.35",
    "mug": "19.99",
    "lamp": "57.80",
    "sticker": "0.29",
}


def price_of(sku: str) -> str:
    """Unit price of `sku`; KeyError for unknown products."""
    return PRICES[sku]
