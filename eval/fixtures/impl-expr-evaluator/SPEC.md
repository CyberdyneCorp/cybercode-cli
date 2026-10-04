# calc language specification

`calc` evaluates single expressions written in a small, dynamically typed language. This
document is the complete contract; where it says "exactly", messages and values must match
character for character.

## Public API

```python
from calc import evaluate, CalcError

evaluate(source: str, env: dict | None = None)
```

- `source` is one expression. `env` maps variable names to values (`None` means no
  variables). `evaluate` never modifies `env`.
- The result is a Python value of one of the five language types (below).
- Every error is raised as `calc.CalcError`, a subclass of `Exception`, with two attributes:
  `message` (str, exactly as specified below) and `column` (int). `str(error)` is
  `f"{message} at column {column}"`. No other exception type may escape `evaluate` for any
  input covered by this specification.
- Columns are 1-based character offsets into `source` (the first character is column 1).
  Newlines are ordinary characters: columns do not restart on a new line.

## Types

| Language type | Python value | Type name in messages |
|---|---|---|
| integer | `int` (arbitrary precision; never `bool`) | `int` |
| decimal | `decimal.Decimal` | `decimal` |
| string | `str` | `str` |
| boolean | `bool` | `bool` |
| null | `None` | `null` |

Integers and decimals are the *numbers*. Booleans are not numbers.

Environment values must be of these Python types (`int`, `bool`, `Decimal`, `str`, `None`);
a Python `bool` in `env` is a boolean, never an integer.

**Truthiness** (used by `and`, `or`, `not` and conditionals): `false`, `null`, integer `0`,
any decimal equal to zero (`0.0`, `-0.00`, ...) and the empty string are falsy; every other
value is truthy.

### Decimal arithmetic

All decimal operations use a context with precision 28 and rounding `ROUND_HALF_EVEN`
(`decimal.Context(prec=28, rounding=decimal.ROUND_HALF_EVEN)`, other settings at their
defaults). The context must be applied locally: `evaluate` must not change the caller's
current decimal context. When an operation mixes an integer and a decimal, the integer is
first converted exactly with `Decimal(n)`. Unless this document says otherwise, a decimal
result is exactly the `Decimal` that the corresponding operation of Python's `decimal` module
produces under that context (so `1.50 + 1` is `Decimal("2.50")` and `1 / 3` is
`Decimal("0.3333333333333333333333333333")`). Tests compare decimal results by type and by
`str()`, so the exponent matters: `Decimal("2.50")` is not an acceptable result where
`Decimal("2.5")` is specified.

## Lexical structure

The source is tokenized completely before parsing begins, so a lexical error anywhere in the
source is reported even if a syntax error comes earlier; the whole expression is parsed before
evaluation begins, so a syntax error is reported even if evaluation would fail earlier.
Lexical and syntax errors therefore never depend on `env`.

- **Whitespace**: space, tab (`\t`), line feed (`\n`) and carriage return (`\r`) separate
  tokens and are otherwise ignored.
- **Identifiers**: an ASCII letter or `_` followed by ASCII letters, digits and `_`.
  Identifiers are case-sensitive. The keywords `true false null and or not if else let in`
  are reserved and are never identifiers.
- **Numbers**: when the lexer sees an ASCII digit, it takes the longest run of characters that
  are ASCII letters, ASCII digits, `_` or `.` as the literal text. The text must match one of

  | Form | Pattern | Value |
  |---|---|---|
  | integer | `INT` | `int` |
  | decimal | `INT.FRAC` | `Decimal(text_without_underscores)` (exact, never rounded) |

  where `INT` is `0` or a digit 1-9 followed by digits, and `FRAC` is one or more digits; in
  both, single underscores may separate two digits (`1_000`, `3.141_592`). Anything else
  (`007`, `00.5`, `1.`, `1__0`, `1_`, `1_.5`, `1._5`, `1.2.3`, `1e5`, `12abc`) is the error
  `invalid number literal '<text>'` at the column of the literal's first character. There is
  no sign in a literal: `-5` is unary minus applied to `5`. A literal cannot start with `.`.
- **Strings**: delimited by `"` or `'`; the closing delimiter must match the opening one, and
  the other quote character may appear unescaped inside. Escapes:

  | Escape | Meaning |
  |---|---|
  | `\\` `\"` `\'` | backslash, double quote, single quote |
  | `\n` `\t` `\r` | line feed, tab, carriage return |
  | `\u{H...}` | the code point with 1 to 6 hex digits `H` (either case), at most `10FFFF` and not a surrogate (`D800`-`DFFF`) |

  Any other character after a backslash is the error `invalid escape sequence '\<c>'`
  (`<c>` is that character) at the column of the backslash. A `\u` escape that is not of the
  form above (no `{`, no digits, more than 6 digits, a non-hex character, a missing `}`, a value
  out of range or a surrogate) is the error `invalid unicode escape` at the column of the
  backslash. A string that reaches a raw line feed or the end of the source before its closing
  delimiter is the error `unterminated string` at the column of the opening delimiter; this
  includes a backslash immediately followed by a line feed or by the end of the source. The
  string is scanned left to right and the first problem met is reported (`"\q` is an invalid
  escape, not an unterminated string).
- **Operators and punctuation**: `**` `//` `==` `!=` `<=` `>=` `+` `-` `*` `/` `%` `<` `>`
  `(` `)` `,` `=`. The longest match wins (`**` is one token, `* *` is two).
- Any other character (including `!` on its own, `.` not inside a number literal, `[`, `$`,
  or a non-ASCII letter outside a string) is the error `unexpected character '<c>'` at its
  column.

## Grammar

From lowest to highest precedence:

```
expr        := let_expr | conditional
let_expr    := "let" IDENT "=" expr "in" expr
conditional := or_expr [ "if" or_expr "else" expr ]
or_expr     := and_expr { "or" and_expr }                 (left-associative)
and_expr    := not_expr { "and" not_expr }                (left-associative)
not_expr    := "not" not_expr | comparison
comparison  := sum { ("==" | "!=" | "<" | "<=" | ">" | ">=") sum }   (chained, see below)
sum         := term { ("+" | "-") term }                  (left-associative)
term        := unary { ("*" | "/" | "//" | "%") unary }   (left-associative)
unary       := ("-" | "+") unary | power
power       := primary [ "**" unary ]                     (right-associative)
primary     := INTEGER | DECIMAL | STRING | "true" | "false" | "null"
             | IDENT | IDENT "(" [ expr { "," expr } ] ")" | "(" expr ")"
```

Consequences worth spelling out:

- `**` binds tighter than a unary operator on its left and accepts a unary operator on its
  right: `-2 ** 2` is `-(2 ** 2)` = `-4`, `2 ** -1` is allowed, `2 ** 3 ** 2` is
  `2 ** (3 ** 2)` = `512`, and `2 ** -2 ** 2` is `2 ** (-(2 ** 2))`.
- `let` and conditionals are only allowed where `expr` is (top level, inside parentheses, as a
  call argument, as a `let` value or body, or as the `else` branch). `1 + let x = 1 in x` is
  a syntax error (`unexpected token 'let'`); write `1 + (let x = 1 in x)`. The `if` operand of
  a conditional is an `or_expr`, so `1 if let x = 1 in x else 2` is a syntax error too.
  `x if a else y if b else z` is `x if a else (y if b else z)`, and `1 + 2 if c else 3` is
  `(1 + 2) if c else 3`.
- A `let` body extends as far right as possible.
- No trailing commas in calls; `f()` with zero arguments is valid syntax.

**Syntax errors.** When the parser needs a token and the next token does not fit the grammar,
the error is `unexpected end of input` at column `len(source) + 1` if there are no tokens
left, otherwise `unexpected token '<text>'` at the token's first column, where `<text>` is the
token's exact source text (for strings, including the quotes and escapes as written). Tokens
remaining after a complete `expr` are reported the same way (`1 2` is
`unexpected token '2'` at column 3). An empty or whitespace-only source is
`unexpected end of input`. A keyword where an identifier is required (`let in = 1 in 2`) is an
unexpected token.

## Evaluation

Evaluation order is strictly left to right: an operator evaluates its left operand completely
before its right operand, and only then checks operand types. Errors raised while evaluating
an operand propagate unchanged.

### Variables and `let`

- An identifier evaluates to its value in the innermost enclosing `let` that binds it, else to
  `env[name]`; otherwise it is the error `undefined variable '<name>'` at the identifier's
  column.
- `let x = V in B` evaluates `V` in the enclosing scope (so `let x = x + 1 in x` reads the
  outer `x`), then evaluates `B` with `x` bound to that value, shadowing any outer `x`
  (including one in `env`). The binding is visible only inside `B`.

### Arithmetic

Unless listed here, an arithmetic operator applied to operand types not allowed below is the
error `unsupported operand types for <op>: <left type> and <right type>` (type names from the
table above) at the column of the operator.

- `+`: two numbers add; two strings concatenate.
- `-`: two numbers subtract.
- `*`: two numbers multiply; a string and an integer (in either order) repeat the string
  (a count of zero or less gives `""`). Repetition with a decimal or boolean count is a type
  error.
- `/`: two numbers; the result is **always a decimal**: `Decimal(a) / Decimal(b)` under the
  context (`6 / 3` is `Decimal("2")`, `1 / 4` is `Decimal("0.25")`).
- `//` and `%`: two numbers, with floor semantics: `a // b` is the greatest integer not above
  the exact quotient, and `a % b` is `a - b * (a // b)` (zero or with the sign of `b`).
  For two integers the result is an `int` (Python's `//` and `%`). If either operand is a
  decimal, both results are decimals computed exactly as follows, with `a` and `b` as
  decimals: `q = a // b` and `r = a % b` using Python's decimal operators (which truncate
  toward zero); then if `r` is non-zero and `r` and `b` have different signs, `q = q - 1`
  and `r = r + b`. `a // b` is `q` and `a % b` is `r` (so `-7.5 // 2` is `Decimal("-4")` and
  `-7.5 % 2` is `Decimal("0.5")`).
- `**`: the base must be a number and the exponent an **integer**; any other combination
  (including a decimal exponent) is a type error. With exponent 0 the result is `1` for an
  integer base and `Decimal("1")` for a decimal base (also for a zero base). An integer base
  with a positive exponent gives an `int`. An integer base with a negative exponent gives
  `Decimal(base) ** exponent` under the context (`2 ** -1` is `Decimal("0.5")`). A decimal
  base gives `base ** exponent` under the context.
- Division by zero: `/`, `//` and `%` with a divisor equal to zero (integer or decimal), and a
  zero base (integer or decimal) with a negative exponent, are the error `division by zero`
  at the column of the operator.
- Unary `-` on an integer gives `-n`; on a decimal it gives `d.copy_negate()` (exact, so `-0.0`
  is `Decimal("-0.0")`). Unary `+` returns a number unchanged. Any other operand is the error
  `bad operand type for unary <op>: <type>` at the column of the operator.

### Comparison

- `==` and `!=` accept any two values and never fail. Two numbers are equal when numerically
  equal (`1 == 1.0` is `true`); two strings when they are the same string; two booleans when
  equal; `null` equals only `null`. Values of different types are never equal: `true == 1`
  and `"1" == 1` are `false`.
- `<`, `<=`, `>`, `>=` compare two numbers numerically or two strings by code point
  (Python's `str` ordering). Any other pair is the error
  `unsupported operand types for <op>: <left type> and <right type>` at the operator's
  column.
- Chaining: `a op1 b op2 c ...` means `(a op1 b) and (b op2 c) and ...`, except that each
  operand is evaluated at most once and evaluation stops at the first comparison that is
  false, so later operands are not evaluated. The result is always a boolean.

### Logic and conditionals

- `a and b` evaluates `a`; if it is falsy the result is `a` itself, otherwise `b` is evaluated
  and is the result. `a or b` evaluates `a`; if it is truthy the result is `a`, otherwise the
  result is `b`. The unused operand is not evaluated. (`0 or "x"` is `"x"`, `2 and 3` is `3`,
  `"" and 1/0` is `""`.)
- `not a` is the boolean negation of the truthiness of `a`.
- `x if c else y` evaluates `c` first; if it is truthy the result is `x`, otherwise `y`. Only
  the chosen branch is evaluated.

### Builtin functions

A call `name(args)` always refers to the builtin named `name`, even when a variable of that
name is in scope; builtins are not values (a bare `len` is just a variable lookup). A call is
processed in this order, all errors at the column of the function name:

1. Unknown name: `unknown function '<name>'`.
2. Wrong argument count (checked before any argument is evaluated):
   `<name>() expects <n> argument(s), got <k>` with the exact wording from the table.
3. The arguments are evaluated left to right.
4. Argument type checks, then the result.

| Function | Arity message | Behavior |
|---|---|---|
| `len(s)` | `len() expects 1 argument, got <k>` | Number of code points of string `s`. Otherwise `len() argument must be str, not <type>`. |
| `abs(x)` | `abs() expects 1 argument, got <k>` | Absolute value of a number (int stays int; a decimal gives `d.copy_abs()`). Otherwise `abs() argument must be a number, not <type>`. |
| `min(a, ...)` / `max(a, ...)` | `min() expects at least 1 argument, got 0` (resp. `max()`) | All arguments must be numbers, or all must be strings; otherwise `min() arguments must be all numbers or all strings` (resp. `max()`). Returns the first argument that is minimal (resp. maximal), unchanged: `min(1, 1.0)` is `1`, `max(2.0, 2)` is `Decimal("2.0")`. |
| `round(x)` / `round(x, n)` | `round() expects 1 or 2 arguments, got <k>` | `x` must be a number, otherwise `round() argument must be a number, not <type>`; `n` must be an integer, otherwise `round() ndigits must be int, not <type>`. Rounding is always half to even. `round(x)` returns an `int`: `x` itself for an integer, otherwise the nearest integer (`round(2.5)` is `2`, `round(-3.5)` is `-4`). `round(x, n)` with integer `x` returns an `int`: `x` itself when `n >= 0`, otherwise `x` rounded to a multiple of `10 ** -n` (`round(25, -1)` is `20`). With decimal `x` it returns `x.quantize(Decimal(1).scaleb(-n), rounding=ROUND_HALF_EVEN)` (`round(2.675, 2)` is `Decimal("2.68")`, `round(1.5, 0)` is `Decimal("2")`, `round(1234.5, -2)` is `Decimal("1.2E+3")`). |
| `str(x)` | `str() expects 1 argument, got <k>` | Strings unchanged; integers in decimal digits (`-12`); decimals as Python's `str(d)` (`1E+2`, `-0.0`); `true`/`false`; `null`. |
| `int(x)` | `int() expects 1 argument, got <k>` | Integers unchanged; a decimal truncated toward zero (`int(-2.7)` is `-2`); a string that matches `[+-]?[0-9]+` exactly (no spaces or underscores, leading zeros allowed) converts to that integer, any other string is `invalid literal for int(): '<s>'` with `<s>` the string's value as is (no escaping); booleans and `null` are `int() argument must be a number or str, not <type>`. |

The arity messages use the singular `argument` only for the fixed count 1:
`len() expects 1 argument, got 2`, `round() expects 1 or 2 arguments, got 0`,
`max() expects at least 1 argument, got 0`.
