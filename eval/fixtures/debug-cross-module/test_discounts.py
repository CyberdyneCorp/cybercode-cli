import unittest

from shop.discounts import apply_bulk_discount


class DiscountTest(unittest.TestCase):
    def test_no_discount_below_ten(self):
        self.assertEqual(apply_bulk_discount(900, 9), 900)

    def test_ten_percent_from_ten_units(self):
        self.assertEqual(apply_bulk_discount(1000, 10), 900)

    def test_discount_rounds_half_up(self):
        self.assertEqual(apply_bulk_discount(1005, 10), 904)


if __name__ == "__main__":
    unittest.main()
