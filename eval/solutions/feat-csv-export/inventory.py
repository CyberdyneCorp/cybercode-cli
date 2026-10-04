"""Load stock items and report on them."""

import argparse
import csv
import json
import os
import sys
from dataclasses import dataclass, fields
from pathlib import Path
from typing import Iterable, TextIO


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


def format_cell(value: object) -> str:
    if isinstance(value, bool):
        return "yes" if value else "no"
    if value is None:
        return ""
    if isinstance(value, float):
        return f"{value:.2f}"
    return str(value)


def export_csv(records: Iterable[Item], out: str | os.PathLike | TextIO,
               columns: list[str] | None = None) -> int:
    """Write `records` as CSV to a path or text file object; return the number of data rows."""
    columns = list(FIELDS if columns is None else columns)
    unknown = [c for c in columns if c not in FIELDS]
    if unknown:
        raise ValueError(f"unknown column: {unknown[0]}")
    if isinstance(out, (str, os.PathLike)):
        with open(out, "w", encoding="utf-8", newline="") as handle:
            return export_csv(records, handle, columns)
    writer = csv.writer(out, lineterminator="\n")
    writer.writerow(columns)
    count = 0
    for record in records:
        writer.writerow([format_cell(getattr(record, c)) for c in columns])
        count += 1
    return count


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Report on stock items.")
    parser.add_argument("items", help="JSON file with the stock items")
    parser.add_argument("--csv", metavar="PATH", help="write a CSV export to PATH instead of the table")
    args = parser.parse_args(argv)
    items = load_items(args.items)
    if args.csv:
        count = export_csv(items, args.csv)
        print(f"wrote {count} rows to {args.csv}")
    else:
        sys.stdout.write(format_table(items))
    return 0


if __name__ == "__main__":
    sys.exit(main())
