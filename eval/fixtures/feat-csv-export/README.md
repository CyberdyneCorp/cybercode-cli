# inventory

`inventory.py` loads stock items from a JSON file and prints them as a table:

    python3 inventory.py items.json

`items.json` is a list of objects with `sku`, `name`, `quantity`, `price`, `discontinued`
and optional `supplier` (see `sample_items.json`). The purchasing team wants a CSV export
they can open in a spreadsheet, both as a function and as a CLI option (see the task
description for the exact format).

Run the tests with `python3 -m unittest`.
