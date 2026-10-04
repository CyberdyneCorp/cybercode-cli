import unittest

from cart import Cart


class CartTest(unittest.TestCase):
    def test_add_item(self):
        cart = Cart("ana")
        cart.add("apple")
        self.assertEqual(cart.items, ["apple"])

    def test_new_cart_is_empty(self):
        self.assertEqual(Cart("bo").total_items(), 0)


if __name__ == "__main__":
    unittest.main()
