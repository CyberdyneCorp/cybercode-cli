import unittest
from decimal import Decimal

from calc import CalcError, evaluate


class CalcTest(unittest.TestCase):
    def test_precedence(self):
        self.assertEqual(evaluate("1 + 2 * 3"), 7)
        self.assertEqual(evaluate("(1 + 2) * 3"), 9)

    def test_division_gives_decimal(self):
        self.assertEqual(evaluate("7 / 2"), Decimal("3.5"))

    def test_strings(self):
        self.assertEqual(evaluate('"ab" + "c"'), "abc")

    def test_variables_and_let(self):
        self.assertEqual(evaluate("x + 1", {"x": 41}), 42)
        self.assertEqual(evaluate("let x = 2 in x * x"), 4)

    def test_division_by_zero(self):
        with self.assertRaises(CalcError) as caught:
            evaluate("1 / 0")
        self.assertEqual((caught.exception.message, caught.exception.column), ("division by zero", 3))


if __name__ == "__main__":
    unittest.main()
