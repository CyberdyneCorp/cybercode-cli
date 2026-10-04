import unittest
from datetime import date
from decimal import Decimal

from billing import BillingService, Catalog
from billing.money import minor_units, round_money
from billing.periods import period_bounds


class RegressionTest(unittest.TestCase):
    def setUp(self):
        self.service = BillingService(Catalog.load())

    def test_minor_units_by_alpha_code(self):
        self.assertEqual((minor_units("JPY"), minor_units("bhd"), minor_units("840")), (0, 3, 2))
        self.assertEqual(str(round_money(Decimal("7.5255"), "BHD")), "7.526")
        with self.assertRaises(ValueError):
            minor_units("XYZ")

    def test_periods_keep_the_anchor_day(self):
        self.assertEqual(period_bounds(date(2024, 1, 31), 1, 2), (date(2024, 3, 31), date(2024, 4, 30)))
        self.assertEqual(period_bounds(date(2024, 2, 29), 12, 4), (date(2028, 2, 29), date(2029, 2, 28)))

    def test_float_rates_are_exact(self):
        catalog = Catalog.from_dict({"jurisdictions": {"X": {"rate": 0.0725}}})
        self.assertEqual(str(catalog.jurisdiction("X").rate), "0.0725")
        sub = self.service.subscribe("c", "team-annual-usd", date(2024, 2, 29), jurisdiction="US-CA")
        self.assertEqual(self.service.period_invoice(sub, 0).tax, Decimal("93.53"))

    def test_proration_rounds_once(self):
        sub = self.service.subscribe("c", "starter-usd", date(2024, 1, 31))
        invoice = self.service.change_plan(sub, "pro-usd", date(2024, 2, 27))
        # 2 of 29 days (Feb 27 and 28): 19.99 * 2 / 29 = 1.3786..., 49.99 * 2 / 29 = 3.4475...
        self.assertEqual([line.amount for line in invoice.lines], [Decimal("-1.38"), Decimal("3.45")])

    def test_coupon_applies_before_tax(self):
        sub = self.service.subscribe("c", "pro-usd", date(2024, 1, 1), coupon="SAVE15", jurisdiction="DE")
        invoice = self.service.period_invoice(sub, 0)
        self.assertEqual((invoice.discount, invoice.tax, invoice.total),
                         (Decimal("7.50"), Decimal("8.07"), Decimal("50.56")))


if __name__ == "__main__":
    unittest.main()
