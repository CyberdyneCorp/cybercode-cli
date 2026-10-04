import unittest

from shop.checkout import checkout


class CheckoutTest(unittest.TestCase):
    def test_single_item(self):
        self.assertEqual(checkout({"pen": 1}), "1.50")

    def test_bulk_discount_order(self):
        # 12 mugs: 239.88, minus 10% (23.99) = 215.89
        self.assertEqual(checkout({"mug": 12}), "215.89")


if __name__ == "__main__":
    unittest.main()
