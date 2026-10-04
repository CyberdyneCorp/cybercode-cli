# invoice

Invoice arithmetic (`invoice.py`, Python 3.10+, stdlib only).

Money rules (the accountant's contract):

- Input amounts and rates are decimal strings such as `"19.99"`, `"0.125"` or `"0.0825"`.
  Quantities are non-negative ints. Returned amounts are strings with exactly two decimals and no
  thousands separator, such as `"1234.57"`.
- All rounding is to whole cents, **half up** (`0.005` becomes `0.01`), never banker's rounding
  and never binary-float artifacts.
- `line_total(unit_price, quantity)`: unit price times quantity, rounded.
- `invoice_total(lines, tax_rate)` with `lines` a list of `(unit_price, quantity)` returns
  `{"subtotal": ..., "tax": ..., "total": ...}`: the subtotal is the sum of the **rounded** line
  totals, the tax is the subtotal times the rate, rounded, and the total is subtotal plus tax.
- `split_amount(total, n)` splits an amount into `n >= 1` shares (otherwise `ValueError`) that
  add up to exactly `total`. Shares differ by at most one cent and the larger shares come first:
  `split_amount("100.00", 3)` is `["33.34", "33.33", "33.33"]`.

Customers report invoices that are one cent off and bill splits that do not add up.
Run the tests with `python3 -m unittest`.
