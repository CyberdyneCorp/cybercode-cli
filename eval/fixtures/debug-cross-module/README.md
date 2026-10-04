# shop

A tiny checkout service (Python 3.10+, stdlib only).

| Module | Responsibility |
|---|---|
| `shop/catalog.py` | Unit prices as decimal strings (`"19.99"`). |
| `shop/money.py` | `to_cents(amount)` converts a decimal string or number with at most two decimals to exact integer cents; `format_cents(cents)` renders non-negative cents as `"215.89"`. |
| `shop/discounts.py` | Bulk discount: 10% off a line of 10 or more units, rounded half up to the cent. |
| `shop/checkout.py` | `checkout(order)` totals an order `{sku: quantity}` and returns the formatted total. |

CI is red: `test_bulk_discount_order` in `test_checkout.py` fails with a total a few cents too
low. Run the tests with `python3 -m unittest`.
