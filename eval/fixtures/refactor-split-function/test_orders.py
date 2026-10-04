import unittest

from orders import process_orders

SAMPLE = """\
# order_id,customer,sku,quantity,unit_price,coupon
o1,alice,MUG,2,7.50
o1,alice,TEA,1,24.00,SAVE10
o2,bob,SPOON,12,1.25,BULK
o3,carol,MUG,zero,7.50
"""

EXPECTED = """\
ORDER REPORT
============
customer alice: 2 lines, subtotal 39.00, discount 2.40, tax 2.93, total 39.53
customer bob: 1 line, subtotal 15.00, discount 0.75, tax 1.14, total 15.39
------------
lines: 3  customers: 2
subtotal 54.00
discount 3.15
tax 4.07
total 54.92
errors (1):
  line 5: quantity must be a positive integer
"""


class ProcessOrdersTest(unittest.TestCase):
    def test_sample_report(self):
        self.assertEqual(process_orders(SAMPLE), EXPECTED)

    def test_empty_input(self):
        report = process_orders("")
        self.assertIn("no valid orders\n", report)
        self.assertTrue(report.endswith("errors: none\n"))


if __name__ == "__main__":
    unittest.main()
