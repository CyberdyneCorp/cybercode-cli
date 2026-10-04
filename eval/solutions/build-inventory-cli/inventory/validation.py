"""Argument validation: each function returns the normalized value or raises InventoryError."""

import re
from decimal import Decimal

from . import invalid

SKU = re.compile(r"[A-Z0-9][A-Z0-9-]{1,19}")
CATEGORY = re.compile(r"[a-z0-9-]{1,20}")
QUANTITY = re.compile(r"[0-9]+")
PRICE = re.compile(r"[0-9]+(\.[0-9]{1,2})?")
DELTA = re.compile(r"[+-]?[0-9]+")
CENTS = Decimal("0.01")


def sku(value: str) -> str:
    if not SKU.fullmatch(value):
        raise invalid("sku", value)
    return value


def name(value: str) -> str:
    stripped = value.strip()
    if not 1 <= len(stripped) <= 60:
        raise invalid("name", value)
    return stripped


def category(value: str) -> str:
    if not CATEGORY.fullmatch(value):
        raise invalid("category", value)
    return value


def quantity(value: str) -> int:
    if not QUANTITY.fullmatch(value):
        raise invalid("quantity", value)
    return int(value)


def price(value: str) -> str:
    if not PRICE.fullmatch(value):
        raise invalid("price", value)
    return str(Decimal(value).quantize(CENTS))


def delta(value: str) -> int:
    if not DELTA.fullmatch(value):
        raise invalid("delta", value)
    return int(value)
