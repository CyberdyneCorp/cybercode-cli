# inventory

A command-line inventory tracker for a small workshop. Python 3.10+, standard library only.
The full contract is in `SPEC.md`.

## Usage

```sh
python3 -m inventory add A-100 "Hex key set" --category tools --quantity 4 --price 12.50
python3 -m inventory update A-100 --name "Hex key set (metric)" --price 13
python3 -m inventory stock A-100 -1
python3 -m inventory show A-100
python3 -m inventory list --category tools --sort value --reverse
python3 -m inventory list --search hex --low-stock 5 --format json
python3 -m inventory report
python3 -m inventory remove A-100
```

The store is `inventory.json` in the current directory; choose another with `--db PATH`
(before the command) or the `INVENTORY_DB` environment variable. Exit codes: 0 success,
1 the command cannot be applied, 2 usage or invalid value, 3 unreadable store.

## Tests

```sh
python3 -m unittest discover -s tests
```
