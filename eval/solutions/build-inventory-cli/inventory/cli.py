"""Argument parsing and command dispatch."""

import argparse
import sys

from . import InventoryError, formatting, not_found, store, validation

SORT_KEYS = {
    "sku": lambda i: (i["sku"],),
    "name": lambda i: (i["name"].lower(), i["sku"]),
    "quantity": lambda i: (i["quantity"], i["sku"]),
    "value": lambda i: (formatting.value(i), i["sku"]),
}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="inventory", description="Workshop inventory tracker.")
    parser.add_argument("--db", help="store path (default: $INVENTORY_DB or ./inventory.json)")
    commands = parser.add_subparsers(dest="command", required=True)

    add = commands.add_parser("add", help="add an item")
    add.add_argument("sku")
    add.add_argument("name")
    add.add_argument("--category", default="general")
    add.add_argument("--quantity", default="0")
    add.add_argument("--price", required=True)

    commands.add_parser("remove", help="remove an item").add_argument("sku")

    update = commands.add_parser("update", help="change an item's fields")
    update.add_argument("sku")
    update.add_argument("--name")
    update.add_argument("--category")
    update.add_argument("--price")

    stock = commands.add_parser("stock", help="adjust an item's quantity")
    stock.add_argument("sku")
    stock.add_argument("delta")

    commands.add_parser("show", help="show one item").add_argument("sku")

    listing = commands.add_parser("list", help="list items")
    listing.add_argument("--category")
    listing.add_argument("--search")
    listing.add_argument("--low-stock")
    listing.add_argument("--sort", choices=sorted(SORT_KEYS), default="sku")
    listing.add_argument("--reverse", action="store_true")
    listing.add_argument("--format", choices=["table", "json"], default="table")

    commands.add_parser("report", help="summarize by category")
    return parser


def cmd_add(args, path):
    item = {
        "sku": validation.sku(args.sku),
        "name": validation.name(args.name),
        "category": validation.category(args.category),
        "quantity": validation.quantity(args.quantity),
        "price": validation.price(args.price),
    }
    items = store.load(path)
    if item["sku"] in items:
        raise InventoryError(f"sku {item['sku']} already exists", 1)
    items[item["sku"]] = item
    store.save(path, items)
    return f"added {item['sku']}"


def cmd_remove(args, path):
    sku = validation.sku(args.sku)
    items = store.load(path)
    if items.pop(sku, None) is None:
        raise not_found(sku)
    store.save(path, items)
    return f"removed {sku}"


def cmd_update(args, path):
    sku = validation.sku(args.sku)
    changes = {}
    if args.name is not None:
        changes["name"] = validation.name(args.name)
    if args.category is not None:
        changes["category"] = validation.category(args.category)
    if args.price is not None:
        changes["price"] = validation.price(args.price)
    if not changes:
        raise InventoryError("nothing to update", 2)
    items = store.load(path)
    if sku not in items:
        raise not_found(sku)
    items[sku].update(changes)
    store.save(path, items)
    return f"updated {sku}"


def cmd_stock(args, path):
    sku = validation.sku(args.sku)
    delta = validation.delta(args.delta)
    items = store.load(path)
    if sku not in items:
        raise not_found(sku)
    have = items[sku]["quantity"]
    if have + delta < 0:
        raise InventoryError(f"insufficient stock for {sku} (have {have}, need {-delta})", 1)
    items[sku]["quantity"] = have + delta
    store.save(path, items)
    return f"{sku} quantity {have + delta}"


def cmd_show(args, path):
    sku = validation.sku(args.sku)
    items = store.load(path)
    if sku not in items:
        raise not_found(sku)
    return formatting.show(items[sku])


def cmd_list(args, path):
    category = validation.category(args.category) if args.category is not None else None
    low_stock = validation.quantity(args.low_stock) if args.low_stock is not None else None
    search = args.search.lower() if args.search is not None else None
    items = [
        item for item in store.load(path).values()
        if (category is None or item["category"] == category)
        and (low_stock is None or item["quantity"] <= low_stock)
        and (search is None or search in item["sku"].lower() or search in item["name"].lower())
    ]
    items.sort(key=SORT_KEYS[args.sort], reverse=args.reverse)
    return formatting.as_json(items) if args.format == "json" else formatting.table(items)


def cmd_report(args, path):
    return formatting.report(list(store.load(path).values()))


COMMANDS = {
    "add": cmd_add, "remove": cmd_remove, "update": cmd_update, "stock": cmd_stock,
    "show": cmd_show, "list": cmd_list, "report": cmd_report,
}


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        print(COMMANDS[args.command](args, store.resolve_path(args.db)))
    except InventoryError as error:
        print(f"error: {error}", file=sys.stderr)
        return error.exit_code
    return 0
