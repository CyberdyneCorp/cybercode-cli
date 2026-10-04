"""Hidden tests for debug-billing-proration.

Expected values in hidden_billing_cases.py come from an independent Fraction-based oracle of
the rules in the fixture's README.md.
"""
import unittest
from datetime import date
from decimal import Decimal

from billing import BillingService, Catalog, render_invoice
from billing import export, money, periods, schedule
from hidden_billing_cases import BOUNDS_CASES, CHANGE_CASES, CUSTOM_CATALOG, INDEX_CASES, PERIOD_CASES

UNITS = {"USD": 2, "EUR": 2, "GBP": 2, "CHF": 2, "JPY": 0, "KRW": 0, "BHD": 3, "KWD": 3}
NUMERIC = {"USD": "840", "EUR": "978", "GBP": "826", "CHF": "756", "JPY": "392", "KRW": "410",
           "BHD": "048", "KWD": "414"}
TAX_PERCENT = {"NONE": "0", "US-CA": "7.25", "US-NY": "8.875", "DE": "19", "GB": "20", "JP": "10", "BH": "10",
               "TX": "8.25", "TN": "9.55", "WA": "6.5", "CH": "8.1", "KR": "10", "ZL": "15", "ZI": "3.5",
               "Z3": "7.25"}


def d(text):
    return date.fromisoformat(text)


def catalogs():
    return {"D": Catalog.load(), "C": Catalog.from_dict(CUSTOM_CATALOG)}


class AmountAssertions(unittest.TestCase):
    def assertAmount(self, actual, expected, currency, what):
        self.assertIsInstance(actual, Decimal, f"{what} must be a Decimal")
        self.assertEqual(actual, Decimal(expected), what)
        self.assertEqual(actual.as_tuple().exponent, -UNITS[currency],
                         f"{what} {actual!s} must be quantized to {UNITS[currency]} decimals")

    def assertInvoice(self, invoice, expected):
        cur = invoice.currency
        got_lines = [(l.kind, l.description, l.start.isoformat(), l.end.isoformat()) for l in invoice.lines]
        want_lines = [(k, desc, s, e) for k, desc, _, s, e in expected["lines"]]
        self.assertEqual(got_lines, want_lines)
        for line, (_, _, amount, _, _) in zip(invoice.lines, expected["lines"]):
            self.assertAmount(line.amount, amount, cur, f"line {line.description!r}")
        for name in ("subtotal", "discount", "tax", "total"):
            self.assertAmount(getattr(invoice, name), expected[name], cur, name)

    def expected_render(self, customer, currency, jurisdiction, coupon, expected):
        def row(label, amount):
            return f"{label:<54} {amount:>12}"
        out = [f"INVOICE {customer} ({currency})"]
        for _, desc, amount, s, e in expected["lines"]:
            out.append(row(f"{s}..{e}  {desc[:30]}", amount))
        out.append(row("Subtotal", expected["subtotal"]))
        if Decimal(expected["discount"]) != 0:
            out.append(row(f"Discount {coupon}", "-" + expected["discount"]))
        out.append(row(f"Tax {jurisdiction} {TAX_PERCENT[jurisdiction]}%", expected["tax"]))
        out.append(row("Total", expected["total"]))
        return "\n".join(out) + "\n"


class MoneyTest(unittest.TestCase):
    def test_minor_units_for_every_accepted_code(self):
        for alpha, units in UNITS.items():
            for code in (alpha, alpha.lower(), NUMERIC[alpha]):
                with self.subTest(code=code):
                    self.assertEqual(money.minor_units(code), units)
                    self.assertEqual(money.normalize_currency(code), alpha)

    def test_unknown_currency_raises(self):
        for code in ("XYZ", "999", "", "US"):
            with self.subTest(code=code):
                with self.assertRaises(ValueError):
                    money.minor_units(code)
                with self.assertRaises(ValueError):
                    money.normalize_currency(code)

    def test_round_money(self):
        cases = [
            ("0.005", "USD", "0.01"), ("-0.005", "USD", "-0.01"), ("0.004999", "USD", "0.00"),
            ("2.675", "USD", "2.68"), ("1.005", "EUR", "1.01"), ("49.99", "USD", "49.99"), ("7", "USD", "7.00"),
            ("0.5", "JPY", "1"), ("1.5", "JPY", "2"), ("2.5", "JPY", "3"), ("-2.5", "JPY", "-3"),
            ("1545.517", "JPY", "1546"), ("980", "JPY", "980"), ("12.49", "KRW", "12"),
            ("0.0005", "BHD", "0.001"), ("7.5255", "BHD", "7.526"), ("18.75", "BHD", "18.750"),
            ("-1.0005", "KWD", "-1.001"), ("3", "KWD", "3.000"), ("0.125", "GBP", "0.13"),
        ]
        for amount, cur, expected in cases:
            with self.subTest(amount=amount, currency=cur):
                result = money.round_money(Decimal(amount), cur)
                self.assertEqual(str(result), expected)

    def test_round_money_rejects_float(self):
        with self.assertRaises(TypeError):
            money.round_money(0.1, "USD")

    def test_format_amount(self):
        cases = [
            ("1078", "JPY", "1078"), ("1078.4", "JPY", "1078"), ("20.625", "BHD", "20.625"),
            ("-7.5", "USD", "-7.50"), ("0", "USD", "0.00"), ("-0.001", "USD", "0.00"), ("-0", "JPY", "0"),
            ("1234567.891", "USD", "1234567.89"), ("12.5", "BHD", "12.500"), ("-0.4", "KRW", "0"),
        ]
        for amount, cur, expected in cases:
            with self.subTest(amount=amount, currency=cur):
                self.assertEqual(money.format_amount(Decimal(amount), cur), expected)


class CatalogTest(unittest.TestCase):
    def test_float_numbers_mean_their_repr(self):
        cat = Catalog.from_dict(CUSTOM_CATALOG)
        self.assertEqual(str(cat.plan("basic-usd").price), "9.99")
        self.assertEqual(str(cat.plan("max-usd").price), "99.95")
        self.assertEqual(str(cat.plan("basic-chf-q").price), "30.05")
        self.assertEqual(str(cat.jurisdiction("TX").rate), "0.0825")
        self.assertEqual(str(cat.jurisdiction("Z3").rate), "0.0725")
        self.assertEqual(str(cat.coupon("P7_5").percent_off), "7.5")
        default = Catalog.load()
        self.assertEqual(str(default.jurisdiction("US-CA").rate), "0.0725")
        self.assertEqual(str(default.jurisdiction("US-NY").rate), "0.08875")

    def test_plan_currencies_are_normalized(self):
        cat = Catalog.from_dict(CUSTOM_CATALOG)
        self.assertEqual(cat.plan("basic-usd").currency, "USD")
        self.assertEqual(cat.plan("lite-krw").currency, "KRW")
        self.assertEqual(cat.plan("plus-kwd").currency, "KWD")


class PeriodTest(unittest.TestCase):
    def test_period_bounds(self):
        for anchor, months, k, start, end in BOUNDS_CASES:
            with self.subTest(anchor=anchor, interval=months, index=k):
                self.assertEqual(periods.period_bounds(d(anchor), months, k), (d(start), d(end)))

    def test_add_months_clamps(self):
        self.assertEqual(periods.add_months(d("2024-01-31"), 1), d("2024-02-29"))
        self.assertEqual(periods.add_months(d("2023-01-31"), 1), d("2023-02-28"))
        self.assertEqual(periods.add_months(d("2024-01-31"), 2), d("2024-03-31"))
        self.assertEqual(periods.add_months(d("2024-02-29"), 12), d("2025-02-28"))
        self.assertEqual(periods.add_months(d("2024-12-31"), -1), d("2024-11-30"))

    def test_period_index(self):
        for anchor, months, on, k in INDEX_CASES:
            with self.subTest(anchor=anchor, interval=months, on=on):
                self.assertEqual(periods.period_index(d(anchor), months, d(on)), k)
        with self.assertRaises(ValueError):
            periods.period_index(d("2024-01-31"), 1, d("2024-01-30"))

    def test_schedule(self):
        self.assertEqual(schedule.upcoming_periods(d("2024-01-31"), 1, 3, on=d("2024-03-30")),
                         [(d("2024-02-29"), d("2024-03-31")), (d("2024-03-31"), d("2024-04-30")),
                          (d("2024-04-30"), d("2024-05-31"))])
        self.assertEqual(schedule.upcoming_periods(d("2024-02-29"), 12, 4),
                         [(d("2024-02-29"), d("2025-02-28")), (d("2025-02-28"), d("2026-02-28")),
                          (d("2026-02-28"), d("2027-02-28")), (d("2027-02-28"), d("2028-02-29"))])
        self.assertEqual(schedule.next_renewal(d("2024-01-31"), 1, d("2024-03-30")), d("2024-03-31"))
        self.assertEqual(schedule.next_renewal(d("2023-08-31"), 3, d("2024-02-29")), d("2024-05-31"))


class PeriodInvoiceTest(AmountAssertions):
    def test_period_invoices(self):
        cats = catalogs()
        for (cat, plan, anchor, k, coupon, jur), expected in PERIOD_CASES:
            with self.subTest(case=f"{cat}:{plan}@{anchor}#{k} {coupon} {jur}"):
                service = BillingService(cats[cat])
                sub = service.subscribe("cust", plan, d(anchor), coupon, jur)
                self.assertInvoice(service.period_invoice(sub, k), expected)

    def test_rendered_period_invoices(self):
        cats = catalogs()
        for (cat, plan, anchor, k, coupon, jur), expected in PERIOD_CASES[::3]:
            with self.subTest(case=f"{cat}:{plan}@{anchor}#{k} {coupon} {jur}"):
                service = BillingService(cats[cat])
                invoice = service.period_invoice(service.subscribe("Ana B", plan, d(anchor), coupon, jur), k)
                want = self.expected_render("Ana B", invoice.currency, jur, coupon, expected)
                self.assertEqual(render_invoice(invoice), want)


class PlanChangeTest(AmountAssertions):
    def test_plan_change_invoices(self):
        cats = catalogs()
        for (cat, old, new, anchor, on, coupon, jur), expected in CHANGE_CASES:
            with self.subTest(case=f"{cat}:{old}->{new}@{anchor} on {on} {coupon} {jur}"):
                service = BillingService(cats[cat])
                sub = service.subscribe("cust", old, d(anchor), coupon, jur)
                self.assertInvoice(service.change_plan(sub, new, d(on)), expected)
                self.assertEqual(sub.plan_code, new)

    def test_rendered_and_exported_change_invoices(self):
        cats = catalogs()
        for (cat, old, new, anchor, on, coupon, jur), expected in CHANGE_CASES[1::2]:
            with self.subTest(case=f"{cat}:{old}->{new}@{anchor} on {on} {coupon} {jur}"):
                service = BillingService(cats[cat])
                invoice = service.change_plan(service.subscribe("Bo", old, d(anchor), coupon, jur), new, d(on))
                self.assertEqual(render_invoice(invoice),
                                 self.expected_render("Bo", invoice.currency, jur, coupon, expected))
                data = export.invoice_to_dict(invoice)
                self.assertEqual(data["lines"], [
                    {"kind": k, "description": desc, "start": s, "end": e, "amount": a}
                    for k, desc, a, s, e in expected["lines"]])
                for name in ("subtotal", "discount", "tax", "total"):
                    self.assertEqual(data[name], expected[name], name)
                self.assertEqual((data["coupon"], data["jurisdiction"]), (coupon, jur))

    def test_next_period_after_change_bills_new_plan(self):
        service = BillingService(Catalog.load())
        sub = service.subscribe("cust", "starter-usd", d("2024-01-31"), "SAVE15", "US-CA")
        service.change_plan(sub, "team-usd", d("2024-03-30"))
        invoice = service.period_invoice(sub, 2)
        self.assertEqual([(l.description, l.start, l.end) for l in invoice.lines],
                         [("Team", d("2024-03-31"), d("2024-04-30"))])
        self.assertEqual((invoice.discount, invoice.tax, invoice.total),
                         (Decimal("19.35"), Decimal("7.95"), Decimal("117.60")))

    def test_invalid_changes(self):
        service = BillingService(Catalog.load())
        sub = service.subscribe("cust", "pro-usd", d("2024-01-31"))
        for new in ("pro-eur", "pro-annual-usd", "pro-usd"):
            with self.subTest(new=new), self.assertRaises(ValueError):
                service.change_plan(sub, new, d("2024-02-10"))
        with self.assertRaises(ValueError):
            service.change_plan(sub, "team-usd", d("2024-01-30"))
        euro = service.subscribe("cust", "pro-eur", d("2024-01-31"), "FIVEBUCKS")
        with self.assertRaises(ValueError):
            service.period_invoice(euro, 0)


if __name__ == "__main__":
    unittest.main()
