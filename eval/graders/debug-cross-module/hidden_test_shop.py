import unittest

from shop.checkout import checkout
from shop.money import format_cents, to_cents


class HiddenMoneyTest(unittest.TestCase):
    def test_to_cents_is_exact(self):
        cases = {
            "19.99": 1999, "0.29": 29, "0.57": 57, "4.35": 435, "1.15": 115, "57.80": 5780,
            "1005.10": 100510, "8.20": 820, "0.07": 7, "0": 0, "7": 700, "12.5": 1250,
            "0.10": 10, "2.01": 201, "9.95": 995, "4.10": 410,
        }
        for amount, cents in cases.items():
            with self.subTest(amount=amount):
                self.assertEqual(to_cents(amount), cents)
                self.assertIsInstance(to_cents(amount), int)

    def test_to_cents_accepts_numbers(self):
        self.assertEqual(to_cents(19.99), 1999)
        self.assertEqual(to_cents(0.29), 29)
        self.assertEqual(to_cents(3), 300)

    def test_every_two_decimal_amount_below_100(self):
        for cents in range(0, 10000):
            amount = f"{cents // 100}.{cents % 100:02d}"
            self.assertEqual(to_cents(amount), cents, amount)

    def test_format_cents(self):
        self.assertEqual(format_cents(21589), "215.89")
        self.assertEqual(format_cents(5), "0.05")
        self.assertEqual(format_cents(0), "0.00")


class HiddenCheckoutTest(unittest.TestCase):
    def test_visible_cases(self):
        self.assertEqual(checkout({"pen": 1}), "1.50")
        self.assertEqual(checkout({"mug": 12}), "215.89")

    def test_orders(self):
        self.assertEqual(checkout({"sticker": 1}), "0.29")
        self.assertEqual(checkout({"notebook": 3, "sticker": 7}), "15.08")
        self.assertEqual(checkout({"mug": 9}), "179.91")
        self.assertEqual(checkout({"mug": 10}), "179.91")
        self.assertEqual(checkout({"lamp": 1, "mug": 2, "sticker": 10}), "100.39")
        self.assertEqual(checkout({}), "0.00")


if __name__ == "__main__":
    unittest.main()
