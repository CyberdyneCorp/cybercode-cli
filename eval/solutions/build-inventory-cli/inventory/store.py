"""JSON store: load, validate shape, and save atomically."""

import json
import os
import re
import tempfile
from pathlib import Path

from . import InventoryError

FIELDS = {"sku": str, "name": str, "category": str, "quantity": int, "price": str}
PRICE = re.compile(r"[0-9]+\.[0-9]{2}")


def resolve_path(cli_value: str | None) -> Path:
    return Path(cli_value or os.environ.get("INVENTORY_DB") or "inventory.json")


def _valid_item(item) -> bool:
    return (
        isinstance(item, dict)
        and set(item) == set(FIELDS)
        and all(type(item[key]) is kind for key, kind in FIELDS.items())
        and item["quantity"] >= 0
        and PRICE.fullmatch(item["price"]) is not None
    )


def load(path: Path) -> dict[str, dict]:
    """Return items keyed by SKU. A missing file is an empty inventory."""
    if not path.exists():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        data = None
    items = data.get("items") if isinstance(data, dict) and data.get("version") == 1 else None
    if not isinstance(items, list) or not all(_valid_item(item) for item in items):
        raise InventoryError(f"cannot read store {path}", 3)
    return {item["sku"]: item for item in items}


def save(path: Path, items: dict[str, dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    document = {"version": 1, "items": [items[sku] for sku in sorted(items)]}
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
            handle.write("\n")
        os.replace(tmp, path)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
