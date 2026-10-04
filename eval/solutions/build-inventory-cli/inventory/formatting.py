"""Output rendering for list, show and report."""

import json
from decimal import Decimal

HEADERS = ("SKU", "NAME", "CATEGORY", "QTY", "PRICE", "VALUE")
RIGHT_ALIGNED = {"QTY", "PRICE", "VALUE"}


def value(item: dict) -> Decimal:
    return item["quantity"] * Decimal(item["price"])


def plural(count: int, word: str) -> str:
    return f"{count} {word}" if count == 1 else f"{count} {word}s"


def show(item: dict) -> str:
    lines = [f"{key}: {item[key]}" for key in ("sku", "name", "category", "quantity", "price")]
    return "\n".join(lines + [f"value: {value(item):.2f}"])


def table(items: list[dict]) -> str:
    if not items:
        return "no items"
    rows = [HEADERS] + [
        (i["sku"], i["name"], i["category"], str(i["quantity"]), i["price"], f"{value(i):.2f}")
        for i in items
    ]
    widths = [max(len(row[col]) for row in rows) for col in range(len(HEADERS))]
    lines = [
        "  ".join(
            cell.rjust(width) if header in RIGHT_ALIGNED else cell.ljust(width)
            for cell, width, header in zip(row, widths, HEADERS)
        ).rstrip()
        for row in rows
    ]
    total = sum((value(i) for i in items), Decimal(0))
    return "\n".join(lines + [f"{plural(len(items), 'item')}, total value {total:.2f}"])


def as_json(items: list[dict]) -> str:
    return json.dumps([{**i, "value": f"{value(i):.2f}"} for i in items], indent=2)


def report(items: list[dict]) -> str:
    categories: dict[str, list[dict]] = {}
    for item in items:
        categories.setdefault(item["category"], []).append(item)
    lines = [summary(name, categories[name]) for name in sorted(categories)]
    return "\n".join(lines + [summary("total", items)])


def summary(label: str, items: list[dict]) -> str:
    units = sum(i["quantity"] for i in items)
    total = sum((value(i) for i in items), Decimal(0))
    return f"{label}: {plural(len(items), 'item')}, {plural(units, 'unit')}, value {total:.2f}"
