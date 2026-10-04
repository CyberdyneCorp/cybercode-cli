# inventory CLI specification

Python 3.10+, standard library only. The program is the package `inventory/`, run as
`python3 -m inventory [--db PATH] <command> [arguments]`. Tests live in `tests/` and run with
`python3 -m unittest discover -s tests`.

## Store

- Path: `--db PATH` (given before the command) if present, else the `INVENTORY_DB` environment
  variable if set and non-empty, else `inventory.json` in the current directory.
- A missing file is an empty inventory; it is created (with its parent directory, if missing)
  on the first command that changes data. Read-only commands never create it.
- Format: UTF-8 JSON object `{"version": 1, "items": [...]}`. Each item is exactly
  `{"sku": str, "name": str, "category": str, "quantity": int, "price": str}`, where `price`
  is a decimal string with exactly two decimals (`"2.50"`). Items are kept sorted by `sku`.
  Write with `indent=2` and a trailing newline. Replace the file atomically (write a temporary
  file in the same directory, then rename it over the store).
- A store that is not valid JSON, or not of the shape above, is an error: print
  `error: cannot read store <path>` to stderr and exit 3. Never overwrite such a file.

## Values

| Field | Rule | Regular expression |
|---|---|---|
| sku | 2–20 characters from `A-Z`, `0-9` and `-`; the first is a letter or digit. Not case-folded. | `[A-Z0-9][A-Z0-9-]{1,19}` |
| name | Leading/trailing whitespace is stripped; then 1–60 characters. | |
| category | 1–20 characters from `a-z`, `0-9` and `-`. Default `general`. | `[a-z0-9-]{1,20}` |
| quantity | A non-negative integer in decimal digits only (`0`, `12`). Default `0`. | `[0-9]+` |
| price | Digits, optionally followed by `.` and one or two decimals (`3`, `3.5`, `3.50`); stored with two decimals. | `[0-9]+(\.[0-9]{1,2})?` |
| delta | An integer with an optional sign (`5`, `+5`, `-3`, `0`). | `[+-]?[0-9]+` |

Regular expressions must match the whole argument. Every SKU argument of every command is
validated, so `show ab` is an invalid SKU, not an unknown one. The `list` filters are validated
too: an invalid `--category` is reported as `invalid category` and an invalid `--low-stock` as
`invalid quantity`.

An invalid value prints `error: invalid <field>: <value>` to stderr (for example
`error: invalid price: 1.234`, `error: invalid sku: ab`) and exits 2 without changing the store.
`<field>` is one of `sku`, `name`, `category`, `quantity`, `price`, `delta`; `<value>` is the
argument exactly as given. Values are validated, in the order the arguments appear in the
command synopsis, before the store is consulted.

Money is exact: use `decimal.Decimal`, never floats. An item's value is `quantity × price`,
printed with two decimals.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | The command is valid but cannot be applied (unknown SKU, duplicate SKU, insufficient stock). |
| 2 | Usage error: unknown command, missing/unknown arguments (argparse's own message is fine), or an invalid value. |
| 3 | The store cannot be read. |

All error messages go to stderr and start with `error: ` (argparse usage errors excepted).
Successful commands print to stdout only.

## Commands

### `add SKU NAME [--category CATEGORY] [--quantity QUANTITY] --price PRICE`

Adds an item and prints `added <SKU>`. If the SKU exists: `error: sku <SKU> already exists`,
exit 1.

### `remove SKU`

Removes an item and prints `removed <SKU>`. Unknown SKU: `error: sku <SKU> not found`, exit 1
(the same message and exit code apply to every command that takes an existing SKU).

### `update SKU [--name NAME] [--category CATEGORY] [--price PRICE]`

Changes the given fields and prints `updated <SKU>`. With none of the options:
`error: nothing to update`, exit 2.

### `stock SKU DELTA`

Adds DELTA to the quantity and prints `<SKU> quantity <new quantity>`. If the result would be
negative: `error: insufficient stock for <SKU> (have <quantity>, need <-DELTA>)`, exit 1.
Example: with quantity 2, `stock A-1 -3` fails with
`error: insufficient stock for A-1 (have 2, need 3)`.

### `show SKU`

Prints six `key: value` lines:

```
sku: A-100
name: Hex key set
category: tools
quantity: 4
price: 12.50
value: 50.00
```

### `list [--category CATEGORY] [--search TEXT] [--low-stock N] [--sort {sku,name,quantity,value}] [--reverse] [--format {table,json}]`

Filters (all combine with AND):

- `--category`: exact category match (validated like a category value).
- `--search`: case-insensitive substring of the SKU or the name.
- `--low-stock N`: quantity `<= N` (N validated like a quantity value).

Sorting: by `sku` (default), `name` (case-insensitive), `quantity` or `value`, ascending, ties
broken by SKU ascending. `--reverse` reverses the final order.

`--format table` (default): columns `SKU`, `NAME`, `CATEGORY`, `QTY`, `PRICE`, `VALUE`. Each
column is as wide as its longest cell (header included); the first three are left-aligned, the
last three right-aligned; columns are separated by two spaces; no line has trailing whitespace.
After the rows comes one line `<n> item(s), total value <sum>` — `1 item` or `<n> items`. With
no matching items, print only `no items`.

```
SKU    NAME         CATEGORY  QTY  PRICE  VALUE
A-100  Hex key set  tools       4  12.50  50.00
B-7    Glue         general    10   3.00  30.00
2 items, total value 80.00
```

`--format json`: a JSON array (indent 2) of objects
`{"sku", "name", "category", "quantity", "price", "value"}` with `price` and `value` as
two-decimal strings, in the same order as the table would be. No matches: `[]`.

### `report`

One line per category, sorted by category name, then a total line:

```
general: 1 item, 10 units, value 30.00
tools: 2 items, 5 units, value 62.50
total: 3 items, 15 units, value 92.50
```

`item`/`items` and `unit`/`units` follow the count (1 is singular). An empty inventory prints
only `total: 0 items, 0 units, value 0.00`.

## Done means

- Every command above behaves exactly as specified.
- `tests/` contains a unittest suite that covers every command, validation errors and the
  store format, and it passes.
- `README.md` has a `## Usage` section with an example of every command.
