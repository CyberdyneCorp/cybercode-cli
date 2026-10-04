# billing

Subscription billing for our SaaS plans (Python 3.10+, standard library only): a pricing
catalog, anniversary billing periods, mid-period plan changes with proration, coupons,
per-jurisdiction tax, and invoice rendering/export.

| Module | Responsibility |
|---|---|
| `billing/money.py` | currencies, minor units, rounding and formatting of `Decimal` amounts |
| `billing/catalog.py` + `billing/catalog.json` | plans, coupons and tax jurisdictions (the pricing export) |
| `billing/periods.py` | anniversary billing periods |
| `billing/schedule.py` | renewal schedule built on `periods.py` |
| `billing/proration.py` | credit/charge amounts for a mid-period plan change |
| `billing/discounts.py` | coupons |
| `billing/tax.py` | tax per jurisdiction |
| `billing/invoice.py` | invoice lines and totals |
| `billing/render.py`, `billing/export.py` | text rendering and JSON export |
| `billing/ledger.py` | per-customer ledger of issued invoices and statements |
| `billing/service.py` | `BillingService`, the facade used by the API layer |
| `billing/cli.py` | `python3 -m billing period|change|schedule ...` |

Run the tests with `python3 -m unittest`.

## Billing rules

These rules are the contract. Every amount the package produces must follow them exactly.

### Money

1. All amounts are `decimal.Decimal`. Binary floats never take part in arithmetic:
   `round_money` raises `TypeError` for a float.
2. Currencies and their minor units: USD, EUR, GBP, CHF have 2 decimals; JPY and KRW have 0;
   BHD and KWD have 3. `money.normalize_currency(code)` accepts the alpha code in any case
   (`"usd"`) or the ISO numeric code (`"840"`, `"392"`, `"048"`, ...) and returns the upper-case
   alpha code. `money.minor_units(code)` returns the number of decimals for any accepted code.
   Both raise `ValueError` for an unknown currency.
3. `money.round_money(amount, currency)` rounds to the currency's minor unit, ties away from
   zero (`ROUND_HALF_UP`): 0.005 USD -> 0.01, -0.005 USD -> -0.01, 0.5 JPY -> 1,
   0.0005 BHD -> 0.001. The result is quantized to the minor unit, so `str()` of it shows exactly
   that many decimals (`"980"` for JPY, `"7.525"` for BHD, `"49.99"` for USD).
4. `money.format_amount(amount, currency)` is the rounded amount with exactly the currency's
   number of decimals and no thousands separator (`"1078"`, `"20.625"`, `"-7.50"`); zero is
   never written with a minus sign.
5. Every amount on an invoice (line amounts, `subtotal`, `discount`, `tax`, `total`) is a
   `Decimal` already rounded and quantized as in rule 3.

### Catalog

6. `Catalog.from_dict(data)` / `Catalog.load(path)` read the pricing export (see
   `billing/catalog.json`). Plan `price`, coupon `percent_off`/`amount_off` and jurisdiction
   `rate` may be JSON strings, ints or floats. Strings and ints are exact; a float stands for the
   decimal number it prints as (its shortest `repr`), so a rate of `0.0725` means exactly
   `Decimal("0.0725")` and a price of `19.99` means exactly `Decimal("19.99")`.
7. A plan has a `code`, `name`, `price`, `currency` and `interval_months` (1, 3 or 12). Plan and
   coupon currencies may be written in any form `normalize_currency` accepts and are stored as
   the upper-case alpha code.

### Billing periods (anniversary billing)

8. A subscription's *anchor* is its start date. `periods.add_months(day, n)` moves `day` by `n`
   months, clamping the day of month to the last day of the target month.
9. Period `k` (0-based) of a plan billed every `m` months is
   `periods.period_bounds(anchor, m, k) == (add_months(anchor, k*m), add_months(anchor, (k+1)*m))`;
   the end is exclusive and equals the next period's start. Every period is computed **from the
   anchor**, never from the previous period, so the anchor day is preserved after a short month:
   an anchor of 2024-01-31 gives periods starting 2024-01-31, 2024-02-29, 2024-03-31,
   2024-04-30, 2024-05-31, ...; an annual plan anchored 2024-02-29 renews on 2025-02-28,
   2026-02-28, 2027-02-28, 2028-02-29.
10. `periods.period_index(anchor, m, on)` is the `k` whose period contains `on`
    (`start <= on < end`); a date before the anchor raises `ValueError`.
11. `schedule.upcoming_periods(anchor, m, count, on=None)` returns `count` consecutive
    `(start, end)` periods beginning with the one containing `on` (period 0 when `on` is None);
    `schedule.next_renewal(anchor, m, on)` is the end of the period containing `on`.

### Invoices

12. `BillingService(catalog).subscribe(customer, plan_code, anchor, coupon=None,
    jurisdiction="NONE")` returns a `Subscription`.
13. `service.period_invoice(sub, k)` bills period `k` on the subscription's current plan: one
    `Line(kind="plan", description=<plan name>, amount=<plan price rounded>, start, end)` with the
    period's bounds.
14. `service.change_plan(sub, new_plan_code, on)` switches the subscription to the new plan on
    date `on` and returns the proration invoice. The new plan must have the same currency and
    interval (otherwise `ValueError`; changing to the current plan, or a date `on` before the
    anchor, is also a `ValueError`).
    Let `[start, end)` be the period containing `on`, `D` the number of days in it and
    `R = (end - on)` in days (the change day is counted, the end is not). The invoice has two
    lines, in this order, both with `start=on` and `end=end`:
    * `Line("credit", "Unused time on <old plan name>", -round(old_price * R / D))`
    * `Line("charge", "Remaining time on <new plan name>", round(new_price * R / D))`

    Each amount is computed from the exact quotient `price * R / D` and rounded exactly once
    (rule 3). Do not round a daily rate first. `D` is the real length of that billing period
    (28, 29, 30 or 31 days for monthly plans, 89-92 for quarterly, 365 or 366 for annual).
15. `subtotal` is the sum of the line amounts.
16. Coupons apply to every invoice of a subscription (period and plan-change invoices) and are
    applied **before tax**. `discount` is a non-negative amount subtracted from the subtotal:
    0 when there is no coupon or the subtotal is <= 0; for a `percent_off` coupon
    `round(subtotal * percent_off / 100)`; for an `amount_off` coupon `min(amount_off, subtotal)`,
    and the coupon's currency must match the invoice currency (otherwise `ValueError`).
17. Tax uses the subscription's jurisdiction: a `rate` (a fraction, `0.19` is 19%) and a `mode`.
    * `mode: "invoice"`: `tax = round((subtotal - discount) * rate)`, rounded once per invoice.
    * `mode: "line"`: every line is taxed and rounded on its own, and the discount is taxed like
      a negative line: `tax = sum(round(line.amount * rate)) - round(discount * rate)`.

    A negative taxable amount (a net credit) gives a negative tax.
18. `total = subtotal - discount + tax`.

### Output

19. `render.render_invoice(invoice)` returns text lines joined by `\n` with a final `\n`:
    * header: `INVOICE <customer> (<currency>)`;
    * one row per line: label `<start>..<end>  <description>` (ISO dates, two spaces, the
      description cut to 30 characters);
    * `Subtotal`; then `Discount <coupon code>` with the negated discount, only when the
      discount is not zero; then `Tax <jurisdiction code> <rate as a percent>%` where the
      percent is the rate times 100 written without trailing zeros (`Tax US-CA 7.25%`,
      `Tax DE 19%`, `Tax NONE 0%`); then `Total`.

    Every row is the label left-justified to 54 characters, one space, and the amount
    (`format_amount`) right-justified to 12 characters.
20. `export.invoice_to_dict(invoice)` returns `customer`, `currency`, `lines` (each with
    `kind`, `description`, `start`, `end`, `amount`), `subtotal`, `coupon` (code or null),
    `discount`, `jurisdiction` (code), `tax` and `total`, with every amount formatted by
    `format_amount` and dates in ISO format.
