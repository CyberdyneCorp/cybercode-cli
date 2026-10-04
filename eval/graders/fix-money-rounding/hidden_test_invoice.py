import unittest
from decimal import Decimal

from invoice import invoice_total, line_total, split_amount


class HiddenInvoiceTest(unittest.TestCase):
    def test_visible_cases(self):
        self.assertEqual(line_total("19.99", 3), "59.97")
        self.assertEqual(line_total("1.005", 1), "1.01")
        self.assertEqual(invoice_total([("2.50", 2)], "0")["total"], "5.00")

    def test_line_total_half_up(self):
        cases = [
            ("1.005", 1, "1.01"), ("2.675", 1, "2.68"), ("0.125", 1, "0.13"), ("0.125", 3, "0.38"),
            ("1.15", 3, "3.45"), ("0.1", 3, "0.30"), ("0.333", 3, "1.00"), ("1234.565", 1, "1234.57"),
            ("19.99", 0, "0.00"), ("0.015", 1, "0.02"), ("8.345", 1, "8.35"), ("10", 2, "20.00"),
        ]
        for price, quantity, expected in cases:
            with self.subTest(price=price, quantity=quantity):
                self.assertEqual(line_total(price, quantity), expected)

    def test_subtotal_sums_rounded_lines(self):
        result = invoice_total([("0.005", 1), ("0.005", 1)], "0")
        self.assertEqual(result, {"subtotal": "0.02", "tax": "0.00", "total": "0.02"})

    def test_tax_rounds_half_up(self):
        self.assertEqual(invoice_total([("10.00", 1)], "0.0825"), {"subtotal": "10.00", "tax": "0.83", "total": "10.83"})
        self.assertEqual(invoice_total([("2.50", 1)], "0.05"), {"subtotal": "2.50", "tax": "0.13", "total": "2.63"})

    def test_realistic_invoice(self):
        lines = [("19.99", 3), ("0.125", 7), ("1.005", 2), ("249.95", 1)]
        # 59.97 + 0.88 + 2.01 + 249.95 = 312.81; tax 7.25% = 22.678725 -> 22.68
        self.assertEqual(invoice_total(lines, "0.0725"), {"subtotal": "312.81", "tax": "22.68", "total": "335.49"})

    def test_many_small_lines_have_no_float_drift(self):
        result = invoice_total([("0.10", 1)] * 1000, "0.2")
        self.assertEqual(result, {"subtotal": "100.00", "tax": "20.00", "total": "120.00"})

    def test_empty_invoice(self):
        self.assertEqual(invoice_total([], "0.1"), {"subtotal": "0.00", "tax": "0.00", "total": "0.00"})

    def test_split_amount(self):
        cases = [
            ("100.00", 3, ["33.34", "33.33", "33.33"]),
            ("0.05", 3, ["0.02", "0.02", "0.01"]),
            ("10.00", 4, ["2.50", "2.50", "2.50", "2.50"]),
            ("0.01", 2, ["0.01", "0.00"]),
            ("7.00", 1, ["7.00"]),
            ("1000.01", 6, ["166.67", "166.67", "166.67", "166.67", "166.67", "166.66"]),
        ]
        for total, n, expected in cases:
            with self.subTest(total=total, n=n):
                shares = split_amount(total, n)
                self.assertEqual(shares, expected)
                self.assertEqual(sum(Decimal(s) for s in shares), Decimal(total))

    def test_split_invalid(self):
        for n in (0, -2):
            with self.subTest(n=n), self.assertRaises(ValueError):
                split_amount("10.00", n)


if __name__ == "__main__":
    unittest.main()
