import unittest

from invoice import invoice_total, line_total


class InvoiceTest(unittest.TestCase):
    def test_line_total_simple(self):
        self.assertEqual(line_total("19.99", 3), "59.97")

    def test_half_cent_rounds_up(self):
        self.assertEqual(line_total("1.005", 1), "1.01")

    def test_invoice_without_tax(self):
        self.assertEqual(invoice_total([("2.50", 2)], "0")["total"], "5.00")


if __name__ == "__main__":
    unittest.main()
