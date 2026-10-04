import unittest
from datetime import datetime, timedelta, timezone

from cronexpr import matches, next_fire

# 2024-01-01 is a Monday; 2024-01-07 is a Sunday.
MONDAY = datetime(2024, 1, 1)
SUNDAY = datetime(2024, 1, 7)


def fires(expr, start, count):
    """The first `count` firing times after `start`."""
    times = []
    for _ in range(count):
        start = next_fire(expr, start)
        times.append(start)
    return times


def minutes_of(expr):
    """Minutes within one hour at which `expr` (with `* * * *` for the rest) fires."""
    hour = datetime(2024, 1, 1, 0, 0)
    return [m for m in range(60) if matches(f"{expr} * * * *", hour.replace(minute=m))]


class FieldSyntax(unittest.TestCase):
    def test_star_single_and_list(self):
        self.assertEqual(minutes_of("*"), list(range(60)))
        self.assertEqual(minutes_of("7"), [7])
        self.assertEqual(minutes_of("0,30,59"), [0, 30, 59])

    def test_ranges_are_inclusive(self):
        self.assertEqual(minutes_of("10-15"), [10, 11, 12, 13, 14, 15])
        self.assertEqual(minutes_of("5-5"), [5])
        self.assertEqual(minutes_of("1-3,58-59"), [1, 2, 3, 58, 59])

    def test_steps(self):
        self.assertEqual(minutes_of("*/15"), [0, 15, 30, 45])
        self.assertEqual(minutes_of("10-50/20"), [10, 30, 50])
        self.assertEqual(minutes_of("10-40/20"), [10, 30])
        self.assertEqual(minutes_of("5/20"), [5, 25, 45])
        self.assertEqual(minutes_of("58/1"), [58, 59])
        self.assertEqual(minutes_of("*/1"), list(range(60)))
        days = [d for d in range(1, 32) if matches("0 0 */2 1 *", datetime(2024, 1, d))]
        self.assertEqual(days, list(range(1, 32, 2)))
        hours = [h for h in range(24) if matches("0 */6 * * *", datetime(2024, 1, 1, h))]
        self.assertEqual(hours, [0, 6, 12, 18])

    def test_whitespace(self):
        self.assertTrue(matches("  5\t 4   * * *  ", datetime(2024, 1, 1, 4, 5)))


class Names(unittest.TestCase):
    def test_month_names(self):
        self.assertTrue(matches("0 0 1 FEB *", datetime(2024, 2, 1)))
        self.assertFalse(matches("0 0 1 FEB *", datetime(2024, 1, 1)))
        self.assertTrue(matches("0 0 1 JAN *", datetime(2024, 1, 1)))
        self.assertTrue(matches("0 0 1 DEC *", datetime(2024, 12, 1)))
        self.assertTrue(matches("0 0 1 12 *", datetime(2024, 12, 1)))
        self.assertTrue(matches("0 0 1 1 *", datetime(2024, 1, 1)))

    def test_day_names_and_ranges(self):
        weekdays = [d for d in range(1, 8) if matches("0 0 * * MON-FRI", datetime(2024, 1, d))]
        self.assertEqual(weekdays, [1, 2, 3, 4, 5])
        self.assertTrue(matches("0 0 * * SUN,SAT", SUNDAY))
        self.assertTrue(matches("0 0 * JAN-MAR *", datetime(2024, 3, 31)))
        self.assertFalse(matches("0 0 * JAN-MAR *", datetime(2024, 4, 1)))
        self.assertTrue(matches("0 0 * * 1-FRI", datetime(2024, 1, 5)))

    def test_names_are_case_insensitive(self):
        self.assertTrue(matches("0 0 * jan mon", MONDAY))
        self.assertTrue(matches("0 0 * Jan Mon", MONDAY))
        self.assertTrue(matches("0 0 * * mon-fri", datetime(2024, 1, 3)))


class DayOfWeek(unittest.TestCase):
    def test_numbers(self):
        for offset in range(7):
            day = SUNDAY + timedelta(days=offset)
            self.assertTrue(matches(f"0 0 * * {offset}", day), offset)
            self.assertFalse(matches(f"0 0 * * {(offset + 1) % 7}", day), offset)

    def test_seven_is_sunday(self):
        self.assertTrue(matches("0 0 * * 7", SUNDAY))
        self.assertFalse(matches("0 0 * * 7", MONDAY))
        self.assertTrue(matches("0 0 * * 5-7", SUNDAY))
        self.assertTrue(matches("0 0 * * 0", SUNDAY))


class DayRule(unittest.TestCase):
    def test_both_restricted_is_or(self):
        self.assertTrue(matches("0 0 13 * FRI", datetime(2024, 1, 5)))   # Friday, not the 13th
        self.assertTrue(matches("0 0 13 * FRI", datetime(2024, 1, 13)))  # Saturday the 13th
        self.assertFalse(matches("0 0 13 * FRI", datetime(2024, 1, 6)))
        self.assertTrue(matches("0 0 */2 * MON", datetime(2024, 1, 8)))  # even day, Monday
        self.assertTrue(matches("0 0 1-31 * MON", datetime(2024, 1, 2)))

    def test_one_restricted_is_and(self):
        self.assertTrue(matches("0 0 13 * *", datetime(2024, 1, 13)))
        self.assertFalse(matches("0 0 13 * *", datetime(2024, 1, 12)))
        self.assertFalse(matches("0 0 * * FRI", datetime(2024, 1, 13)))
        self.assertTrue(matches("0 0 * * FRI", datetime(2024, 1, 12)))

    def test_star_with_step_is_restricted(self):
        # */2 restricts the day of month, so with Monday the rule is OR: day 8 is even and Monday,
        # day 2 is even (a Tuesday), day 15 is a Monday but odd, day 3 is neither.
        self.assertTrue(matches("0 0 */2 * 1", datetime(2024, 1, 15)))
        self.assertTrue(matches("0 0 * * */3", datetime(2024, 1, 3)))  # Wednesday = 3
        self.assertFalse(matches("0 0 * * */3", datetime(2024, 1, 2)))
        self.assertTrue(matches("0 0 1 * */7", datetime(2024, 1, 1)))
        self.assertTrue(matches("0 0 1 * */7", SUNDAY))

    def test_next_fire_uses_or_rule(self):
        self.assertEqual(next_fire("0 0 13 * FRI", MONDAY), datetime(2024, 1, 5))
        self.assertEqual(fires("0 0 13 * FRI", datetime(2024, 1, 10), 3),
                         [datetime(2024, 1, 12), datetime(2024, 1, 13), datetime(2024, 1, 19)])
        self.assertEqual(next_fire("0 0 13 * *", MONDAY), datetime(2024, 1, 13))


class Matching(unittest.TestCase):
    def test_seconds_are_ignored(self):
        self.assertTrue(matches("0 12 * * *", datetime(2024, 1, 1, 12, 0, 59, 500000)))
        self.assertFalse(matches("0 12 * * *", datetime(2024, 1, 1, 12, 1)))

    def test_all_fields(self):
        self.assertTrue(matches("30 8 15 6 *", datetime(2024, 6, 15, 8, 30)))
        self.assertFalse(matches("30 8 15 6 *", datetime(2024, 6, 15, 9, 30)))
        self.assertFalse(matches("30 8 15 6 *", datetime(2024, 7, 15, 8, 30)))


class Macros(unittest.TestCase):
    def assertSameAs(self, macro, expr):
        start = datetime(2023, 12, 30, 17, 45)
        self.assertEqual(fires(macro, start, 4), fires(expr, start, 4))

    def test_macros(self):
        self.assertSameAs("@yearly", "0 0 1 1 *")
        self.assertSameAs("@annually", "0 0 1 1 *")
        self.assertSameAs("@monthly", "0 0 1 * *")
        self.assertSameAs("@weekly", "0 0 * * 0")
        self.assertSameAs("@daily", "0 0 * * *")
        self.assertSameAs("@midnight", "0 0 * * *")
        self.assertSameAs("@hourly", "0 * * * *")
        self.assertEqual(next_fire("@weekly", MONDAY), SUNDAY)

    def test_unknown_or_uppercase_macros(self):
        for expr in ["@reboot", "@Daily", "@HOURLY", "@"]:
            with self.assertRaises(ValueError, msg=expr):
                matches(expr, MONDAY)


class NextFire(unittest.TestCase):
    def test_strictly_after(self):
        self.assertEqual(next_fire("0 12 * * *", datetime(2024, 1, 1, 12, 0)), datetime(2024, 1, 2, 12, 0))
        self.assertEqual(next_fire("* * * * *", datetime(2024, 1, 1, 10, 0)), datetime(2024, 1, 1, 10, 1))

    def test_truncates_seconds(self):
        result = next_fire("* * * * *", datetime(2024, 1, 1, 10, 0, 30, 123))
        self.assertEqual(result, datetime(2024, 1, 1, 10, 1))
        self.assertEqual((result.second, result.microsecond), (0, 0))
        self.assertEqual(next_fire("0 12 * * *", datetime(2024, 1, 1, 12, 0, 59)), datetime(2024, 1, 2, 12, 0))

    def test_rollovers(self):
        self.assertEqual(next_fire("0 9 * * *", datetime(2024, 1, 1, 9, 30)), datetime(2024, 1, 2, 9, 0))
        self.assertEqual(next_fire("15 9 * * *", datetime(2024, 1, 1, 9, 30)), datetime(2024, 1, 2, 9, 15))
        self.assertEqual(next_fire("* * * * *", datetime(2024, 12, 31, 23, 59)), datetime(2025, 1, 1, 0, 0))
        self.assertEqual(next_fire("30 * * * *", datetime(2024, 1, 31, 23, 45)), datetime(2024, 2, 1, 0, 30))
        self.assertEqual(next_fire("0 0 31 * *", datetime(2024, 4, 1)), datetime(2024, 5, 31))
        self.assertEqual(fires("*/20 10 * * *", datetime(2024, 1, 1, 10, 20), 3),
                         [datetime(2024, 1, 1, 10, 40), datetime(2024, 1, 2, 10, 0), datetime(2024, 1, 2, 10, 20)])

    def test_leap_days(self):
        self.assertEqual(next_fire("0 0 29 2 *", datetime(2023, 3, 1)), datetime(2024, 2, 29))
        self.assertEqual(next_fire("0 0 29 2 *", datetime(2096, 3, 1)), datetime(2104, 2, 29))
        self.assertEqual(next_fire("0 0 29 2 *", datetime(2096, 2, 29, 0, 0)), datetime(2104, 2, 29))
        self.assertEqual(next_fire("0 0 * 2 *", datetime(2100, 2, 28, 0, 0)), datetime(2101, 2, 1))

    def test_never_fires(self):
        for expr in ["0 0 30 2 *", "0 0 31 4 *", "0 0 31 2,4,6,9,11 *"]:
            with self.assertRaises(ValueError, msg=expr):
                next_fire(expr, MONDAY)


class Validation(unittest.TestCase):
    INVALID = [
        "", "* * * *", "* * * * * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "* * 32 * *",
        "* * * 0 *", "* * * 13 *", "* * * * 8", "*/0 * * * *", "1-5/0 * * * *", "50-10 * * * *",
        "* * * * SAT-SUN", "1,,2 * * * *", "1, * * * *", "abc * * * *", "-5 * * * *", "1- * * * *",
        "*/ * * * *", "** * * * *", "MON * * * *", "* * * MON *", "* * * * JAN", "* * * * JUL",
        "* * JAN * *", "*/MON * * * *", "* * * * MO",
    ]

    def test_invalid_expressions(self):
        for expr in self.INVALID:
            with self.assertRaises(ValueError, msg=expr):
                matches(expr, MONDAY)
            with self.assertRaises(ValueError, msg=expr):
                next_fire(expr, MONDAY)

    def test_boundaries_are_valid(self):
        self.assertTrue(matches("59 23 31 12 7", datetime(2023, 12, 31, 23, 59)))
        self.assertTrue(matches("0 0 1 1 0", datetime(2023, 1, 1)))

    def test_aware_datetimes_rejected(self):
        aware = datetime(2024, 1, 1, tzinfo=timezone.utc)
        with self.assertRaises(ValueError):
            matches("* * * * *", aware)
        with self.assertRaises(ValueError):
            next_fire("* * * * *", aware)


if __name__ == "__main__":
    unittest.main()
