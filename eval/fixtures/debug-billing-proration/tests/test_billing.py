import unittest
from datetime import date
from decimal import Decimal

from billing import BillingService, Catalog, render_invoice


class PeriodInvoiceTest(unittest.TestCase):
    def setUp(self):
        self.service = BillingService(Catalog.load())

    def test_monthly_usd_invoice(self):
        sub = self.service.subscribe("acme", "pro-usd", date(2024, 3, 15))
        invoice = self.service.period_invoice(sub, 0)
        self.assertEqual(invoice.total, Decimal("49.99"))
        self.assertEqual(
            render_invoice(invoice),
            "INVOICE acme (USD)\n"
            "2024-03-15..2024-04-15  Pro                                   49.99\n"
            "Subtotal                                                      49.99\n"
            "Tax NONE 0%                                                    0.00\n"
            "Total                                                         49.99\n",
        )

    def test_jpy_invoice_has_no_fractional_yen(self):
        sub = self.service.subscribe("tanaka", "starter-jpy", date(2024, 4, 1), jurisdiction="JP")
        invoice = self.service.period_invoice(sub, 0)
        self.assertEqual(
            render_invoice(invoice),
            "INVOICE tanaka (JPY)\n"
            "2024-04-01..2024-05-01  Starter                                 980\n"
            "Subtotal                                                        980\n"
            "Tax JP 10%                                                       98\n"
            "Total                                                          1078\n",
        )


if __name__ == "__main__":
    unittest.main()
