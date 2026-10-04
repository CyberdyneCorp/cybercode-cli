import unittest

import util
from cart import Cart
from util import add_tag, with_defaults

DEFAULTS = {"retries": 3, "timeout": 10}


class HiddenUtilTest(unittest.TestCase):
    def test_add_tag_without_list_is_fresh_each_call(self):
        self.assertEqual(add_tag("a"), ["a"])
        self.assertEqual(add_tag("b"), ["b"])
        self.assertEqual(add_tag("c"), ["c"])

    def test_add_tag_does_not_modify_argument(self):
        tags = ["x"]
        result = add_tag("y", tags)
        self.assertEqual(result, ["x", "y"])
        self.assertEqual(tags, ["x"])
        self.assertIsNot(result, tags)

    def test_add_tag_existing(self):
        tags = ["x", "y"]
        result = add_tag("x", tags)
        self.assertEqual(result, ["x", "y"])
        self.assertEqual(tags, ["x", "y"])
        result.append("z")
        self.assertEqual(tags, ["x", "y"])

    def test_with_defaults_is_fresh_each_call(self):
        first = with_defaults({"retries": 9})
        self.assertEqual(first, {"retries": 9, "timeout": 10})
        self.assertEqual(with_defaults(), DEFAULTS)
        self.assertEqual(with_defaults({"verbose": True}), {**DEFAULTS, "verbose": True})
        self.assertEqual(with_defaults(None), DEFAULTS)
        self.assertEqual(util.DEFAULT_OPTIONS, DEFAULTS)

    def test_with_defaults_does_not_modify_inputs(self):
        options = {"timeout": 1}
        result = with_defaults(options)
        result["retries"] = 0
        self.assertEqual(options, {"timeout": 1})
        self.assertEqual(util.DEFAULT_OPTIONS, DEFAULTS)
        self.assertEqual(with_defaults(), DEFAULTS)
        with_defaults()["timeout"] = 99
        self.assertEqual(with_defaults(), DEFAULTS)


class HiddenCartTest(unittest.TestCase):
    def test_visible_cases(self):
        cart = Cart("ana")
        cart.add("apple")
        self.assertEqual(cart.items, ["apple"])
        self.assertEqual(Cart("bo").total_items(), 0)

    def test_carts_do_not_share_items(self):
        first, second = Cart("ana"), Cart("bo")
        first.add("apple")
        second.add("pear")
        self.assertEqual(first.items, ["apple"])
        self.assertEqual(second.items, ["pear"])
        self.assertEqual(Cart("cy").items, [])

    def test_cart_copies_caller_items(self):
        items = ["milk"]
        cart = Cart("ana", items)
        cart.add("bread")
        self.assertEqual(items, ["milk"])
        self.assertEqual(cart.items, ["milk", "bread"])
        other = Cart("bo", items)
        self.assertEqual(other.items, ["milk"])

    def test_tags_are_independent(self):
        tags = ["vip"]
        first = Cart("ana", tags=tags)
        first.tag("gift")
        second = Cart("bo")
        second.tag("new")
        self.assertEqual(tags, ["vip"])
        self.assertEqual(first.tags, ["vip", "gift"])
        self.assertEqual(second.tags, ["new"])
        self.assertEqual(Cart("cy").tags, [])
        first.tag("gift")
        self.assertEqual(first.tags, ["vip", "gift"])

    def test_options_are_independent(self):
        options = {"retries": 1}
        first = Cart("ana", options=options)
        first.options["timeout"] = 99
        second = Cart("bo")
        self.assertEqual(first.options, {"retries": 1, "timeout": 99})
        self.assertEqual(second.options, DEFAULTS)
        self.assertEqual(options, {"retries": 1})
        self.assertEqual(Cart("cy", options={"timeout": 5}).options, {"retries": 3, "timeout": 5})
        self.assertEqual(Cart("dee").options, DEFAULTS)


if __name__ == "__main__":
    unittest.main()
