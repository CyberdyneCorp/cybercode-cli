# cronexpr

`cronexpr.py` is a small, correct cron-expression library (Python 3.10+, standard library
only) with no tests yet. It needs a unittest suite in `test_cronexpr.py`, run with
`python3 -m unittest`.

## Public API

- `matches(expr: str, dt: datetime) -> bool`: whether the expression fires at the minute of
  `dt`.
- `next_fire(expr: str, after: datetime) -> datetime`: the first firing time strictly after
  `after`.

Both raise `ValueError` for an invalid expression (every rule below that says "invalid").
All datetimes are naive (`tzinfo is None`); passing a timezone-aware `dt` or `after` raises
`ValueError`. Dates follow the Gregorian calendar (2100 is not a leap year).

## Expressions

An expression is either a macro or exactly five fields separated by one or more spaces or tabs
(leading and trailing whitespace is ignored); fewer or more than five fields is invalid.

| # | Field | Allowed values | Names |
|---|---|---|---|
| 1 | minute | 0-59 | |
| 2 | hour | 0-23 | |
| 3 | day of month | 1-31 | |
| 4 | month | 1-12 | `JAN`-`DEC` (JAN = 1 ... DEC = 12) |
| 5 | day of week | 0-7 (0 and 7 are both Sunday) | `SUN`-`SAT` (SUN = 0, MON = 1 ... SAT = 6) |

A field is a comma-separated list of one or more items (an empty item, as in `1,,2` or `1,`,
is invalid). An item is one of:

- `*`: every allowed value of the field;
- `N`: the single value N;
- `A-B`: every value from A to B, **both inclusive**; A greater than B is invalid
  (`50-10`, `SAT-SUN`);
- any of the three followed by `/S`, a step S >= 1 (`/0` is invalid): `*/S` is the field's
  minimum, minimum + S, minimum + 2S, ... up to the field's maximum; `A-B/S` is A, A + S,
  A + 2S, ... up to and including B when reached (`10-50/20` is 10, 30, 50); `N/S` is N,
  N + S, ... up to the field's maximum (minute `5/20` is 5, 25, 45).

Numbers are unsigned decimal integers; a number outside the field's allowed values is
invalid (minute `60`, hour `24`, day of month `0` or `32`, month `0` or `13`, day of week `8`).
Month and day-of-week fields also accept their three-letter names wherever a number may appear
(`JAN`, `MON-FRI`, `SUN,SAT`), case-insensitively (`jan`, `Mon`); a step is always a number.
Names are invalid in any other field, and a month name is invalid in the day-of-week field and
vice versa. Anything else (`abc`, `-5`, `1-`, `*/`, `**`) is invalid.

### Macros

| Macro | Equivalent |
|---|---|
| `@yearly`, `@annually` | `0 0 1 1 *` |
| `@monthly` | `0 0 1 * *` |
| `@weekly` | `0 0 * * 0` |
| `@daily`, `@midnight` | `0 0 * * *` |
| `@hourly` | `0 * * * *` |

Macros are lowercase only; any other text starting with `@` (`@reboot`, `@Daily`) is invalid.

## Matching

`matches(expr, dt)` looks only at `dt`'s minute, hour, day, month and weekday; seconds and
microseconds are ignored (`12:00:59.5` matches `0 12 * * *`).

The minute, hour and month fields must always match. The day fields follow the classic
(Vixie) cron rule:

- A day field is *unrestricted* when it is exactly `*` (with no step); anything else,
  including `*/2` or `1-31`, is *restricted*.
- If **both** the day-of-month and the day-of-week fields are restricted, a day matches when
  **either** of them matches (`0 0 13 * FRI` fires on every 13th and on every Friday).
- Otherwise (at most one of them is restricted) a day matches when **both** match, which
  means the restricted one decides (`0 0 13 * *` fires only on the 13th; `0 0 * * FRI` only on
  Fridays).

## next_fire

`next_fire(expr, after)` first truncates `after` to the whole minute (seconds and
microseconds set to zero), then returns the earliest datetime strictly later than that minute
which `matches` the expression. The result always has `second == 0` and `microsecond == 0`,
so `next_fire("* * * * *", datetime(2024, 1, 1, 10, 0, 30))` is `10:01:00` and
`next_fire("0 12 * * *", datetime(2024, 1, 1, 12, 0))` is noon on January 2.

The search covers the years `after.year` through `after.year + 8` inclusive. If no matching
minute exists in that range, `next_fire` raises `ValueError` (for example `0 0 30 2 *` never
fires). The range is long enough for every satisfiable expression: `0 0 29 2 *` after
`2096-03-01` fires at `2104-02-29 00:00`, because 2100 is not a leap year.
