"""Load stock items and report on them."""

import argparse
import json
import sys
from dataclasses import dataclass, fields
from pathlib import Path


@dataclass
class Item:
    sku: str
    name: str
    quantity: int
    price: float
    discontinued: bool
    supplier: str | None = None


FIELDS = [f.name for f in fields(Item)]


def load_items(path: str | Path) -> list[Item]:
    raw = json.loads(Path(path).read_text(encoding="utf-8"))
    return [
        Item(
            sku=entry["sku"],
            name=entry["name"],
            quantity=int(entry["quantity"]),
            price=float(entry["price"]),
            discontinued=bool(entry["discontinued"]),
            supplier=entry.get("supplier"),
        )
        for entry in raw
    ]


def format_table(items: list[Item]) -> str:
    rows = [("SKU", "NAME", "QTY", "PRICE")]
    rows += [(i.sku, i.name, str(i.quantity), f"{i.price:.2f}") for i in items]
    widths = [max(len(row[col]) for row in rows) for col in range(4)]
    return "".join("  ".join(cell.ljust(w) for cell, w in zip(row, widths)).rstrip() + "\n" for row in rows)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Report on stock items.")
    parser.add_argument("items", help="JSON file with the stock items")
    args = parser.parse_args(argv)
    sys.stdout.write(format_table(load_items(args.items)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
