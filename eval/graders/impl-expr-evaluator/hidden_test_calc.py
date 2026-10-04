"""Hidden table-driven tests for calc.evaluate (see the fixture's SPEC.md)."""

import decimal
import unittest
from decimal import Decimal as D

import calc
from calc import CalcError, evaluate


class Err:
    def __init__(self, message: str, column: int):
        self.message = message
        self.column = column

    def __repr__(self):
        return f"Err({self.message!r}, {self.column})"


def same_value(actual, expected) -> bool:
    if type(actual) is not type(expected):
        return False
    if isinstance(expected, D):
        return str(actual) == str(expected)
    return actual == expected


class TableTest(unittest.TestCase):
    def check(self, cases, env=None):
        for case in cases:
            source, expected = case[0], case[1]
            case_env = case[2] if len(case) > 2 else env
            with self.subTest(source=source, env=case_env):
                if isinstance(expected, Err):
                    with self.assertRaises(CalcError) as caught:
                        evaluate(source, case_env)
                    self.assertEqual((caught.exception.message, caught.exception.column),
                                     (expected.message, expected.column))
                    self.assertEqual(str(caught.exception), f"{expected.message} at column {expected.column}")
                else:
                    actual = evaluate(source, case_env)
                    self.assertTrue(same_value(actual, expected),
                                    f"{source!r}: got {actual!r} ({type(actual).__name__}), expected {expected!r}")


class Literals(TableTest):
    def test_numbers(self):
        self.check([
            ("0", 0), ("42", 42), ("1_000_000", 1000000),
            ("123456789012345678901234567890123", 123456789012345678901234567890123),
            ("0.5", D("0.5")), ("3.140", D("3.140")), ("1_0.2_5", D("10.25")),
            ("0.000", D("0.000")),
            ("1.23456789012345678901234567890123", D("1.23456789012345678901234567890123")),
            ("  7  ", 7), ("\t\n\r8\n", 8),
        ])

    def test_invalid_numbers(self):
        self.check([
            ("007", Err("invalid number literal '007'", 1)),
            ("00.5", Err("invalid number literal '00.5'", 1)),
            ("1 + 01", Err("invalid number literal '01'", 5)),
            ("1.", Err("invalid number literal '1.'", 1)),
            ("1__0", Err("invalid number literal '1__0'", 1)),
            ("1_", Err("invalid number literal '1_'", 1)),
            ("1_.5", Err("invalid number literal '1_.5'", 1)),
            ("1._5", Err("invalid number literal '1._5'", 1)),
            ("1.2.3", Err("invalid number literal '1.2.3'", 1)),
            ("2 * 1e5", Err("invalid number literal '1e5'", 5)),
            ("12abc + 1", Err("invalid number literal '12abc'", 1)),
            ("0x1F", Err("invalid number literal '0x1F'", 1)),
            (".5", Err("unexpected character '.'", 1)),
            ("_1", Err("undefined variable '_1'", 1)),
        ])

    def test_strings(self):
        self.check([
            ('"hello"', "hello"), ("'hi'", "hi"), ('""', ""),
            ('"it\'s"', "it's"), ("'say \"x\"'", 'say "x"'),
            (r'"a\nb\tc\rd"', "a\nb\tc\rd"), (r'"\\"', "\\"), (r'"\""', '"'), (r"'\''", "'"),
            (r'"\u{41}\u{e9}"', "Aé"), (r'"\u{1F600}"', "\U0001F600"), (r'"\u{10FFFF}"', "\U0010FFFF"),
            (r'"\u{0}"', "\x00"), ('"café ü"', "café ü"),
            ('"a" + \'b\'', "ab"),
        ])

    def test_string_errors(self):
        self.check([
            (r'"\q"', Err("invalid escape sequence '\\q'", 2)),
            (r'1 + "ab\x"', Err("invalid escape sequence '\\x'", 8)),
            (r'"\u41"', Err("invalid unicode escape", 2)),
            (r'"\u{}"', Err("invalid unicode escape", 2)),
            (r'"\u{1234567}"', Err("invalid unicode escape", 2)),
            (r'"\u{110000}"', Err("invalid unicode escape", 2)),
            (r'"\u{D800}"', Err("invalid unicode escape", 2)),
            (r'"\u{12G}"', Err("invalid unicode escape", 2)),
            (r'"\u{41"', Err("invalid unicode escape", 2)),
            ('"abc', Err("unterminated string", 1)),
            ("1 + 'abc\"", Err("unterminated string", 5)),
            ('"ab\ncd"', Err("unterminated string", 1)),
            ('"ab\\', Err("unterminated string", 1)),
            ('"ab\\\ncd"', Err("unterminated string", 1)),
            ('"\\q', Err("invalid escape sequence '\\q'", 2)),
        ])

    def test_keywords_and_characters(self):
        self.check([
            ("true", True), ("false", False), ("null", None),
            ("True", Err("undefined variable 'True'", 1)),
            ("1 ! 2", Err("unexpected character '!'", 3)),
            ("a $ b", Err("unexpected character '$'", 3)),
            ("[1]", Err("unexpected character '['", 1)),
            ("café", Err("unexpected character 'é'", 4)),
            ("1 +\n  @", Err("unexpected character '@'", 7)),
        ])


class Precedence(TableTest):
    def test_arithmetic_precedence(self):
        self.check([
            ("1 + 2 * 3", 7), ("(1 + 2) * 3", 9), ("10 - 4 - 3", 3), ("2 * 3 % 4", 2),
            ("100 // 10 // 3", 3), ("2 ** 3 ** 2", 512), ("(2 ** 3) ** 2", 64),
            ("-2 ** 2", -4), ("(-2) ** 2", 4), ("2 ** -1", D("0.5")), ("- -3", 3), ("-+-3", 3),
            ("2 ** -2 ** 2", D("0.0625")), ("-3 ** 2 * 2", -18), ("2 * -3", -6),
            ("1 - -1", 2), ("+5", 5), ("10 - 2 ** 3", 2), ("7 % 3 ** 2", 7),
            ("1 + 2 if false else 3", 3), ("1 + 2 if true else 3", 3),
        ])

    def test_logic_precedence(self):
        self.check([
            ("not 1 == 2", True), ("not true and false", False), ("true or false and false", True),
            ("(true or false) and false", False), ("not not 5", True),
            ("1 < 2 == true", False), ("(1 < 2) == true", True),
            ("1 if 0 else 2 if 0 else 3", 3), ("1 if 1 else 2 if 0 else 3", 1),
            ("let x = 1 in x + 1 if x else 0", 2), ("1 + 2 == 3 and 2 * 2 == 4", True),
        ])


class Arithmetic(TableTest):
    def test_integers(self):
        self.check([
            ("7 // 2", 3), ("-7 // 2", -4), ("7 // -2", -4), ("-7 // -2", 3),
            ("7 % 3", 1), ("-7 % 3", 2), ("7 % -3", -2), ("-7 % -3", -1), ("6 % 3", 0),
            ("2 ** 100", 2 ** 100), ("99999999999999999999 * 99999999999999999999", 99999999999999999999 ** 2),
            ("0 ** 0", 1), ("5 ** 0", 1), ("0 ** 5", 0), ("(-2) ** 3", -8),
            ("10 ** -2", D("0.01")), ("3 ** -1", D("0.3333333333333333333333333333")),
            ("(-2) ** -2", D("0.25")),
        ])

    def test_division(self):
        self.check([
            ("6 / 3", D("2")), ("1 / 4", D("0.25")), ("1 / 3", D("0.3333333333333333333333333333")),
            ("2 / 3", D("0.6666666666666666666666666667")), ("-1 / 8", D("-0.125")),
            ("1.0 / 4", D("0.25")), ("7.50 / 2.5", D("3.0")), ("1 / 0.01", D("1E+2")),
            ("10 / 4", D("2.5")), ("0 / 5", D("0")),
            ("1 / 0", Err("division by zero", 3)), ("1 // 0", Err("division by zero", 3)),
            ("5 % 0", Err("division by zero", 3)), ("1.5 / 0.0", Err("division by zero", 5)),
            ("1 % 0.00", Err("division by zero", 3)), ("2 // -0.0", Err("division by zero", 3)),
            ("0 ** -1", Err("division by zero", 3)), ("0.0 ** -2", Err("division by zero", 5)),
            ("1 + 2 / (3 - 3)", Err("division by zero", 7)),
        ])

    def test_decimals(self):
        self.check([
            ("1.50 + 1", D("2.50")), ("0.1 + 0.2", D("0.3")), ("0.1 * 3", D("0.3")),
            ("1.10 * 2", D("2.20")), ("2.5 - 2.5", D("0.0")), ("1 - 1.000", D("0.000")),
            ("-0.0", D("-0.0")), ("-1.5", D("-1.5")), ("+1.50", D("1.50")),
            ("1.5 ** 2", D("2.25")), ("2.0 ** 0", D("1")), ("0.0 ** 0", D("1")), ("1.1 ** -1", D("0.9090909090909090909090909091")),
            ("-7.5 // 2", D("-4")), ("-7.5 % 2", D("0.5")), ("7.5 // 2", D("3")), ("7.5 % 2", D("1.5")),
            ("7.50 % 2", D("1.50")), ("7 // 2.0", D("3")), ("-7 % 2.0", D("1.0")),
            ("7.5 % -2", D("-0.5")), ("-7.5 % -2", D("-1.5")), ("6.0 % 3", D("0.0")), ("-6.0 // 3", D("-2")),
            ("1234567890123456789012345678.9 + 0.1", D("1234567890123456789012345679")),
            ("0.1111111111111111111111111111 * 3", D("0.3333333333333333333333333333")),
            ("2 ** 0.5", Err("unsupported operand types for **: int and decimal", 3)),
            ("12345678901234567890123456789 + 0.0", D("1.234567890123456789012345679E+28")),
        ])

    def test_strings(self):
        self.check([
            ('"ab" * 3', "ababab"), ('3 * "ab"', "ababab"), ('"ab" * 0', ""), ('"ab" * -2', ""),
            ('"a" + "b" + "c"', "abc"), ('"x" * 2 + "y"', "xxy"),
            ('"ab" * 1.0', Err("unsupported operand types for *: str and decimal", 6)),
            ('"ab" * true', Err("unsupported operand types for *: str and bool", 6)),
            ('"a" + 1', Err("unsupported operand types for +: str and int", 5)),
            ('1 + "a"', Err("unsupported operand types for +: int and str", 3)),
            ('"a" - "b"', Err("unsupported operand types for -: str and str", 5)),
            ('"a" * "b"', Err("unsupported operand types for *: str and str", 5)),
            ('"4" / 2', Err("unsupported operand types for /: str and int", 5)),
        ])

    def test_type_errors(self):
        self.check([
            ("true + 1", Err("unsupported operand types for +: bool and int", 6)),
            ("null * 2", Err("unsupported operand types for *: null and int", 6)),
            ("1 - null", Err("unsupported operand types for -: int and null", 3)),
            ("2.5 // false", Err("unsupported operand types for //: decimal and bool", 5)),
            ("5 % true", Err("unsupported operand types for %: int and bool", 3)),
            ("2 ** true", Err("unsupported operand types for **: int and bool", 3)),
            ('"a" ** 2', Err("unsupported operand types for **: str and int", 5)),
            ("1.5 ** 1.5", Err("unsupported operand types for **: decimal and decimal", 5)),
            ("-true", Err("bad operand type for unary -: bool", 1)),
            ('+"a"', Err("bad operand type for unary +: str", 1)),
            ("1 + -null", Err("bad operand type for unary -: null", 5)),
            ("1 +\n  true", Err("unsupported operand types for +: int and bool", 3)),
            ("x - 2", Err("unsupported operand types for -: str and int", 3), {"x": "s"}),
            ("1 + 2 + true", Err("unsupported operand types for +: int and bool", 7)),
            ("(1 / 0) + true", Err("division by zero", 4)),
            ("true + (1 / 0)", Err("division by zero", 11)),
        ])


class Comparison(TableTest):
    def test_equality(self):
        self.check([
            ("1 == 1.0", True), ("1 != 1.00", False), ("0.10 == 0.1", True), ("true == 1", False),
            ('"1" == 1', False), ("null == null", True), ("null == false", False), ("0 == false", False),
            ('"a" == "a"', True), ('"a" != "b"', True), ("true == true", True), ("1 == 2", False),
            ("1 / 3 == 0.3333333333333333333333333333", True), ("-0.0 == 0", True),
            ("null != 0", True), ('"" == null', False),
        ])

    def test_ordering(self):
        self.check([
            ("1 < 2", True), ("2 <= 2.0", True), ("3 > 2.5", True), ("2.5 >= 3", False),
            ('"apple" < "banana"', True), ('"Z" < "a"', True), ('"abc" < "abd"', True), ('"ab" < "a"', False),
            ('"" < "a"', True),
            ('1 < "2"', Err("unsupported operand types for <: int and str", 3)),
            ("true < false", Err("unsupported operand types for <: bool and bool", 6)),
            ("null >= 1", Err("unsupported operand types for >=: null and int", 6)),
            ('"a" > 1.5', Err("unsupported operand types for >: str and decimal", 5)),
            ("1 <= null", Err("unsupported operand types for <=: int and null", 3)),
        ])

    def test_chaining(self):
        self.check([
            ("1 < 2 < 3", True), ("1 < 3 < 2", False), ("3 > 2 > 1", True), ("1 < 2 > 0", True),
            ("1 == 1 == 1", True), ("1 == 1 == true", False), ("1 < 2 <= 2 < 3", True),
            ("2 < 1 < 1 / 0", False),
            ('2 < 1 < "x"', False),
            ('1 < 2 < "x"', Err("unsupported operand types for <: int and str", 7)),
            ("1 < 2 < 3 < 1 / 0", Err("division by zero", 15)),
            ("5 > 4 > 3 > 2 > 1", True), ("5 > 4 > 3 > 3 > 1 / 0", False),
            ("1 < x < 3", True, {"x": 2}), ("1 < x < 3", False, {"x": 3}),
            ("let b = 2 in 1 < b < 3", True),
        ])


class Logic(TableTest):
    def test_and_or(self):
        self.check([
            ('0 or "x"', "x"), ("2 and 3", 3), ('"" and 1 / 0', ""), ("1 or 1 / 0", 1),
            ("0 and 1 / 0", 0), ("null or false", False), ("false or null", None),
            ("0.0 or 7", 7), ('"a" and "b"', "b"), ("0.00 and 1", D("0.00")),
            ("-0.0 or 2", 2), ("1 / 0 or 1", Err("division by zero", 3)),
            ("true and 1 / 0", Err("division by zero", 12)), ('"" or ""', ""),
            ("1 and 2 and 3", 3), ("0 or 0.0 or null", None), ("1 and 0 or 5", 5),
        ])

    def test_not(self):
        self.check([
            ("not 0", True), ("not 1", False), ('not ""', True), ('not "a"', False), ("not null", True),
            ("not 0.0", True), ("not -0.000", True), ("not 0.1", False), ("not not null", False),
            ("not 1 / 0", Err("division by zero", 7)),
        ])

    def test_conditional(self):
        self.check([
            ("1 if true else 2", 1), ("1 if 0 else 2", 2), ('"y" if "" else "n"', "n"),
            ("1 if true else 1 / 0", 1), ("1 / 0 if false else 2", 2),
            ("1 if 1 / 0 else 2", Err("division by zero", 8)),
            ("(1 if false else 2) * 10", 20), ("1 if null else 2.5", D("2.5")),
            ("x if x else 0", 0, {"x": 0}), ("x if x else 0", 5, {"x": 5}),
        ])


class Variables(TableTest):
    def test_env(self):
        self.check([
            ("x + y", 5, {"x": 2, "y": 3}), ("price * 2", D("5.00"), {"price": D("2.50")}),
            ("flag and name", "bob", {"flag": True, "name": "bob"}), ("n", None, {"n": None}),
            ("flag + 1", Err("unsupported operand types for +: bool and int", 6), {"flag": True}),
            ("_a1 * 2", 4, {"_a1": 2}), ("Ab", 1, {"Ab": 1, "ab": 2}),
            ("missing", Err("undefined variable 'missing'", 1), {"x": 1}),
            ("1 + y", Err("undefined variable 'y'", 5)), ("len", 3, {"len": 3}),
            ("len(len)", 2, {"len": "ab"}),
        ])

    def test_let(self):
        self.check([
            ("let x = 2 in x * 3", 6), ("let x = 1 in let y = x + 1 in x + y", 3),
            ("let x = 1 in let x = x + 10 in x", 11), ("let x = x + 1 in x", 6, {"x": 5}),
            ("(let x = 1 in x) + x", 11, {"x": 10}), ("let x = 2 in x if x else 0", 2),
            ("1 + (let x = 1 in x)", 2), ("let a = let b = 2 in b * b in a + 1", 5),
            ("let x = 1 in x + let y = 2 in y", Err("unexpected token 'let'", 18)),
            ("(let x = 1 in x) + y", Err("undefined variable 'y'", 20)),
            ("let x = 1 / 0 in 5", Err("division by zero", 11)),
            ("let s = \"ab\" in s * 2 + s", "ababab"),
            ("max(let x = 3 in x, 2)", 3), ("let x = 1 in (let x = 2 in x) + x", 3),
            ("let f = 1 in f(2)", Err("unknown function 'f'", 14)),
        ])

    def test_env_is_not_modified(self):
        env = {"x": 1}
        self.assertEqual(evaluate("let x = 5 in x", env), 5)
        self.assertEqual(env, {"x": 1})
        self.assertEqual(evaluate("let y = 2 in x + y", env), 3)
        self.assertEqual(env, {"x": 1})
        self.assertIsNone(evaluate("null", None))


class Builtins(TableTest):
    def test_len_abs_str_int(self):
        self.check([
            ('len("hello")', 5), ('len("")', 0), ('len("\\u{1F600}a")', 2), ('len("café")', 4),
            ("len(5)", Err("len() argument must be str, not int", 1)),
            ("len(null)", Err("len() argument must be str, not null", 1)),
            ("abs(-5)", 5), ("abs(-2.50)", D("2.50")), ("abs(-0.0)", D("0.0")), ("abs(3)", 3),
            ('abs("x")', Err("abs() argument must be a number, not str", 1)),
            ("abs(true)", Err("abs() argument must be a number, not bool", 1)),
            ("str(42)", "42"), ("str(-12)", "-12"), ("str(1.50)", "1.50"), ("str(1 / 0.01)", "1E+2"),
            ("str(true)", "true"), ("str(false)", "false"), ("str(null)", "null"), ('str("x")', "x"),
            ("str(-0.0)", "-0.0"), ('str(1) + str(2)', "12"),
            ('int("42")', 42), ('int("-007")', -7), ('int("+3")', 3), ("int(2.7)", 2), ("int(-2.7)", -2),
            ("int(5)", 5), ("int(0.999)", 0),
            ('int(" 4")', Err("invalid literal for int(): ' 4'", 1)),
            ('int("1_000")', Err("invalid literal for int(): '1_000'", 1)),
            ('int("4.0")', Err("invalid literal for int(): '4.0'", 1)),
            ('int("")', Err("invalid literal for int(): ''", 1)),
            ("int(true)", Err("int() argument must be a number or str, not bool", 1)),
            ("int(null)", Err("int() argument must be a number or str, not null", 1)),
            ("1 + int(\"x\")", Err("invalid literal for int(): 'x'", 5)),
        ])

    def test_min_max(self):
        self.check([
            ("min(3, 1, 2)", 1), ("max(3, 1, 2)", 3), ("min(1, 1.0)", 1), ("min(1.0, 1)", D("1.0")),
            ("max(2.0, 2)", D("2.0")), ("max(2, 2.00)", 2), ("min(5)", 5), ('max("b", "a", "c")', "c"),
            ('min("b", "a")', "a"), ("min(-1, -1.5)", D("-1.5")), ("max(1, 2.5, 2)", D("2.5")),
            ('min(1, "a")', Err("min() arguments must be all numbers or all strings", 1)),
            ("max(true, false)", Err("max() arguments must be all numbers or all strings", 1)),
            ("max(null)", Err("max() arguments must be all numbers or all strings", 1)),
            ("min()", Err("min() expects at least 1 argument, got 0", 1)),
            ("2 * max()", Err("max() expects at least 1 argument, got 0", 5)),
        ])

    def test_round(self):
        self.check([
            ("round(2.5)", 2), ("round(3.5)", 4), ("round(-2.5)", -2), ("round(-3.5)", -4),
            ("round(2.4999)", 2), ("round(7)", 7), ("round(0.5)", 0), ("round(1.5)", 2),
            ("round(2.675, 2)", D("2.68")), ("round(2.665, 2)", D("2.66")), ("round(1.5, 0)", D("2")),
            ("round(1.25, 1)", D("1.2")), ("round(1.35, 1)", D("1.4")), ("round(1.2, 3)", D("1.200")),
            ("round(1234.5, -2)", D("1.2E+3")), ("round(-0.125, 2)", D("-0.12")),
            ("round(25, -1)", 20), ("round(35, -1)", 40), ("round(-25, -1)", -20), ("round(149, -2)", 100),
            ("round(150, -2)", 200), ("round(250, -2)", 200), ("round(7, 2)", 7), ("round(-15, -1)", -20),
            ("round(5, -1)", 0), ("round(4, -3)", 0),
            ('round("1")', Err("round() argument must be a number, not str", 1)),
            ("round(1.5, 1.0)", Err("round() ndigits must be int, not decimal", 1)),
            ("round(1.5, true)", Err("round() ndigits must be int, not bool", 1)),
            ("round()", Err("round() expects 1 or 2 arguments, got 0", 1)),
            ("round(1, 2, 3)", Err("round() expects 1 or 2 arguments, got 3", 1)),
        ])

    def test_call_errors(self):
        self.check([
            ("len()", Err("len() expects 1 argument, got 0", 1)),
            ('len("a", "b")', Err("len() expects 1 argument, got 2", 1)),
            ("abs(1, 2)", Err("abs() expects 1 argument, got 2", 1)),
            ("str()", Err("str() expects 1 argument, got 0", 1)),
            ("int(1, 2, 3)", Err("int() expects 1 argument, got 3", 1)),
            ("foo(1)", Err("unknown function 'foo'", 1)),
            ("1 + sqrt(4)", Err("unknown function 'sqrt'", 5)),
            ("foo(1 / 0)", Err("unknown function 'foo'", 1)),
            ("len(1 / 0, 2)", Err("len() expects 1 argument, got 2", 1)),
            ("len(1 / 0)", Err("division by zero", 7)),
            ("max(1, 1 / 0)", Err("division by zero", 10)),
            ("min(\"a\", 1 / 0)", Err("division by zero", 12)),
            ("LEN(\"a\")", Err("unknown function 'LEN'", 1)),
            ("abs (-1)", 1), ("max(1, max(2, 3))", 3),
        ])


class Syntax(TableTest):
    def test_syntax_errors(self):
        self.check([
            ("", Err("unexpected end of input", 1)), ("   ", Err("unexpected end of input", 4)),
            ("1 +", Err("unexpected end of input", 4)), ("(1 + 2", Err("unexpected end of input", 7)),
            ("1 2", Err("unexpected token '2'", 3)), ("1 + * 2", Err("unexpected token '*'", 5)),
            (")", Err("unexpected token ')'", 1)), ("(1))", Err("unexpected token ')'", 4)),
            ("1 + )", Err("unexpected token ')'", 5)), ("x = 1", Err("unexpected token '='", 3)),
            ("let in = 1 in 2", Err("unexpected token 'in'", 5)),
            ("let x 1 in x", Err("unexpected token '1'", 7)),
            ("let x = 1 x", Err("unexpected token 'x'", 11)),
            ("let x = 1", Err("unexpected end of input", 10)),
            ("let 5 = 1 in 2", Err("unexpected token '5'", 5)),
            ("1 if true", Err("unexpected end of input", 10)),
            ("1 if true 2", Err("unexpected token '2'", 11)),
            ("1 if let x = 1 in x else 2", Err("unexpected token 'let'", 6)),
            ("max(1,)", Err("unexpected token ')'", 7)), ("max(,1)", Err("unexpected token ','", 5)),
            ("max(1 2)", Err("unexpected token '2'", 7)), ("(1, 2)", Err("unexpected token ','", 3)),
            ('"a" "b"', Err("unexpected token '\"b\"'", 5)),
            (r'1 "a\n"', Err("unexpected token '\"a\\n\"'", 3)),
            ("1 * * 2", Err("unexpected token '*'", 5)), ("2 ** ** 2", Err("unexpected token '**'", 6)),
            ("not", Err("unexpected end of input", 4)), ("1 and", Err("unexpected end of input", 6)),
            ("1 < < 2", Err("unexpected token '<'", 5)), ("else", Err("unexpected token 'else'", 1)),
            ("1 +\n\n2 3", Err("unexpected token '3'", 8)), ("in", Err("unexpected token 'in'", 1)),
            ("f(", Err("unexpected end of input", 3)), ("()", Err("unexpected token ')'", 2)),
            ("1 = = 2", Err("unexpected token '='", 3)), ("true false", Err("unexpected token 'false'", 6)),
        ])

    def test_error_phases(self):
        # Lexing completes before parsing, and parsing before evaluation.
        self.check([
            ("1 + ) $", Err("unexpected character '$'", 7)),
            ("1 / 0 +", Err("unexpected end of input", 8)),
            ("undefined_name 2", Err("unexpected token '2'", 16)),
            ("(1 +) + 007", Err("invalid number literal '007'", 9)),
            ('1 / 0 + "abc', Err("unterminated string", 9)),
            ("1 / 0 + x", Err("division by zero", 3)),
            ("x + 1 / 0", Err("undefined variable 'x'", 1)),
        ])

    def test_operators_tokenize_longest_match(self):
        self.check([
            ("2**3", 8), ("2* *3", Err("unexpected token '*'", 4)), ("7//2", 3), ("7/ /2", Err("unexpected token '/'", 4)),
            ("1<=1", True), ("1< =1", Err("unexpected token '='", 4)), ("1!=2", True), ("1==1", True),
            ("1>=2", False), ("-1+-1", -2), ("(1)(2)", Err("unexpected token '('", 4)),
        ])


class ErrorType(unittest.TestCase):
    def test_calc_error_shape(self):
        self.assertTrue(issubclass(CalcError, Exception))
        self.assertIs(calc.CalcError, CalcError)
        error = CalcError("boom", 3)
        self.assertEqual((error.message, error.column, str(error)), ("boom", 3, "boom at column 3"))

    def test_decimal_context_is_local(self):
        with decimal.localcontext() as ctx:
            ctx.prec = 5
            ctx.rounding = decimal.ROUND_DOWN
            self.assertEqual(str(evaluate("2 / 3")), "0.6666666666666666666666666667")
            self.assertEqual((ctx.prec, ctx.rounding), (5, decimal.ROUND_DOWN))
            self.assertEqual(str(evaluate("round(2.5)")), "2")
        self.assertEqual(decimal.getcontext().prec, 28)


if __name__ == "__main__":
    unittest.main()
