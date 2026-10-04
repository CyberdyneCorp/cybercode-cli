# ledger CLI specification

A plain-text double-entry accounting tool. Python 3.10+, standard library only. The program is
the package `ledger/`, run as

```
python3 -m ledger -f FILE [--strict] COMMAND [ARGUMENTS]
```

The global options `-f FILE` (required) and `--strict` come before the command. After the
command, its options and PATTERN arguments may be given in any order
(`balance assets --depth 1 food` has the patterns `assets` and `food`). Tests live in
`tests/` and run with `python3 -m unittest discover -s tests`.

All arithmetic is exact: use `decimal.Decimal`, never floats. Values are rounded only when they
are displayed.

## 1. Journal files

A journal is a UTF-8 text file. Lines are split on `\n`; a trailing `\r` is removed from every
line. A line is **blank** when it is empty or contains only spaces and tabs. A line is
**indented** when it is not blank and starts with a space or a tab; otherwise a non-blank line
starts at **column 0**.

A column-0 line is, by its first character:

| First character | Meaning |
|---|---|
| `;` or `#` | a comment line, ignored |
| an ASCII digit `0`-`9` | a transaction header (section 1.1) |
| anything else | a directive (section 1.4) |

An **entry** is a column-0 line that is not a comment, together with the indented lines that
immediately follow it. An entry ends at the next blank line or the next column-0 line (a
column-0 comment line also ends it). Only transactions have indented lines: an indented line
that is not part of a transaction entry (it follows a blank line, a comment line or a directive,
or starts the file) is an error, `unexpected indented line`, reported for each such line.

**Inline comments.** On a transaction header, a posting line and a directive line, a `;` and
everything after it is a comment and is removed before the line is parsed (so payees, account
names and include paths cannot contain `;`). After removing it, trailing whitespace is stripped.

### 1.1 Transactions

```
2024-01-05 * Grocery Store      ; inline comment, dropped
    ; a transaction comment line
    Expenses:Food        45.10 USD
    Assets:Bank
```

**Header.** The first whitespace-separated token is the date, which must be `YYYY-MM-DD`
(exactly four, two and two ASCII digits) naming a real calendar date; otherwise the error is
`invalid date: TOKEN` (TOKEN as written, e.g. `invalid date: 2024-02-30`,
`invalid date: 2024/01/05`). The rest of the line, stripped, may begin with a **status**: a `*`
(cleared) or `!` (pending) that stands alone as a word (followed by whitespace or the end of the
line). Without one the transaction has no status. What remains, stripped, is the **payee**; it
is kept exactly (inner whitespace included). An empty payee is the error `missing payee`.

**Indented lines** of a transaction are, after leading whitespace is removed:

- a **transaction comment** if they start with `;`. Its text is everything after the `;`,
  stripped. Comment lines are kept in order among the postings (`print` reproduces them).
- a **posting** otherwise.

**Postings.** After removing the leading whitespace and the inline comment, the posting is split
at the first occurrence of two consecutive spaces or of a tab: the part before is the account
name, the part after (stripped) is the amount field. Without such a separator the whole text is
the account name and the posting has no amount.

- **Account names** are one or more components separated by `:`. Each component must be
  non-empty and must not start or end with a space (single spaces inside are fine:
  `Expenses:Eating Out`). Otherwise: `invalid account name: NAME`. Names are case-sensitive.
- **Amount field**: split on whitespace, it must be exactly one of
  - `NUMBER COMMODITY`
  - `NUMBER COMMODITY @ PRICE COMMODITY2` (a per-unit cost)
  - `NUMBER COMMODITY @@ TOTAL COMMODITY2` (a total cost)

  Otherwise the error is `invalid amount: TEXT` where TEXT is the amount field (after inline
  comment removal, stripped, as written). The same error applies when a cost number (PRICE or
  TOTAL) is negative or when COMMODITY2 equals COMMODITY.
- **NUMBER**: an optional `-`, then either plain digits or digits grouped by commas in threes
  (`1,234,567`), then optionally `.` and one or more digits. Regular expression:
  `-?([0-9]{1,3}(,[0-9]{3})+|[0-9]+)(\.[0-9]+)?`. So `1,234.50` is valid; `1,23`, `1234,567`,
  `.5`, `5.`, `+5` are not.
- **COMMODITY**: one or more ASCII uppercase letters, `[A-Z]+`.

Regular expressions in this spec must match the whole token.

**Weights and balancing.** Each posting with an amount has a weight:

- without a cost: its amount;
- with `@ PRICE C2`: quantity × PRICE, in C2;
- with `@@ TOTAL C2`: TOTAL in C2 when the quantity is positive, −TOTAL when it is negative,
  0 when it is zero.

The **residual** is the sum of all weights, per commodity. A transaction is checked in this
order, each error reported once at the header line:

1. fewer than two postings: `transaction has fewer than two postings`;
2. more than one posting without an amount: `multiple postings without amount`;
3. if one posting has no amount, its amount is **inferred** as the negated residual. This
   requires exactly one commodity with a non-zero residual. If every commodity's residual is
   zero: `cannot infer amount: transaction already balances`; if two or more are non-zero:
   `cannot infer amount: residual in multiple commodities`. An inferred amount has no cost;
4. otherwise every commodity's residual must be exactly zero, else
   `transaction does not balance: residual R` where R lists each non-zero residual as
   `NUMBER COMMODITY`, sorted by commodity and joined by `, `. These NUMBERs are the exact
   sums in plain notation: an optional `-`, no thousands separators, and exactly as many
   decimals as the exact computation produces (a sum has as many decimals as its most precise
   term, a product as many as both factors together; trailing zeros are kept). Example:
   `10.00 USD` and `-9.990 USD` give `residual 0.010 USD`; `3 X @ 1.10 USD` against
   `-3.2 USD` gives `residual 0.10 USD`.

So different commodities can only be mixed in one transaction through costs.

### 1.2 Display precision and number formatting

The **display precision** of a commodity is the largest number of decimals written for it
anywhere in the journal: in posting amounts, in cost amounts and in the NUMBER of `P`
directives (which counts for COMMODITY2, see 1.4). A commodity written only with integers, or
appearing only as the priced COMMODITY of `P` directives, has precision 0. It is computed over
the whole journal, independent of any report filters. Inferred amounts and computed values
(costs multiplied out, converted values) do not contribute.

Every amount a report displays is formatted as: the quantity rounded to the commodity's display
precision with ROUND_HALF_UP (halves away from zero); an optional `-`; the integer part with a
`,` between each group of three digits; if the precision is above 0, a `.` and exactly that
many decimals; a space; the commodity. Examples at precision 2: `1,234,567.80 USD`,
`-0.50 USD`. A value that rounds to zero is printed without a sign (`0.00 USD`). A **balance**
(a sum over possibly several commodities) is displayed as one formatted amount per commodity
whose exact sum is non-zero, sorted by commodity; a balance with no such commodity is displayed
as the single string `0`.

### 1.3 Includes and reading order

`include PATH` reads another journal at that point, as if its lines were inserted there. PATH
is the rest of the directive line, stripped; a relative PATH is relative to the directory of
the file that contains the directive. An empty PATH is the error `invalid include directive`.

- A file that cannot be read: `cannot read include: PATH` (PATH as written).
- Including a file that is already being read (the file itself or any file that included it,
  directly or indirectly, up to the main file) is the error `include cycle: PATH` (PATH as
  written) and nothing is read. Two paths name the same file when `os.path.realpath` of both
  is equal. Including the same file twice in sequence (not a cycle) reads it twice.

**Display names.** The main file's display name is the `-f` argument exactly as given. An
included file's display name is `os.path.join(os.path.dirname(D), PATH)` where D is the display
name of the including file (no normalisation; an absolute PATH gives PATH itself). Example:
with `-f books/main.journal`, `include sub/a.journal` displays as `books/sub/a.journal`, and an
`include ../b.journal` inside that one as `books/sub/../b.journal`.

**Reading order** is the order in which lines are read, with an included file's lines at the
point of its `include`. Transactions with the same date keep reading order wherever reports
sort by date.

### 1.4 Directives

The directive name is the first whitespace-separated token and is case-sensitive.

| Directive | Meaning |
|---|---|
| `include PATH` | section 1.3 |
| `account NAME` | declares an account. NAME is the rest of the line, stripped. Empty: `invalid account directive`; a bad name: `invalid account name: NAME`. Declaring an account twice is fine. |
| `P DATE COMMODITY NUMBER COMMODITY2` | a market price: on DATE one unit of COMMODITY is worth NUMBER COMMODITY2. The line must have exactly these five whitespace-separated tokens, else `invalid price directive`. Then, in this order: a bad DATE gives `invalid date: DATE`; a bad COMMODITY gives `invalid commodity: COMMODITY`; a bad or negative NUMBER or a bad COMMODITY2 gives `invalid amount: NUMBER COMMODITY2`; COMMODITY2 equal to COMMODITY gives `invalid price directive`. |

Any other directive name is the error `unknown directive: NAME` (for example `p` or
`commodity`).

### 1.5 Strict mode

With `--strict`, every posting account must be declared by an `account` directive somewhere in
the journal (before or after its use, in any file). Each posting of an otherwise valid
transaction whose account is not declared exactly (declaring `Assets` does not declare
`Assets:Bank`, nor the reverse) is the error `undeclared account: NAME`, reported at the
posting's line, once per such posting.

## 2. Errors and exit codes

| Code | Meaning |
|---|---|
| 0 | Success. Output goes to stdout; nothing is written to stderr. |
| 1 | The journal cannot be read or is invalid. |
| 2 | Usage error: missing/unknown command or option (argparse's own message is fine), or an invalid option value. |

Errors go to stderr; when a command fails nothing is written to stdout.

**Journal errors** are printed as `error: FILE:LINE: MESSAGE`, with FILE the display name and
LINE the 1-based line number in that file. Parsing never stops at the first error:

- an error in a transaction header or posting line discards the whole transaction; the rest
  of its entry is skipped without further errors. A transaction reports at most one error: the
  first bad line, else the first failing check of 1.1 (at the header line);
- an erroneous directive is ignored; processing continues with the next line;
- strict-mode errors are reported only for transactions without any other error.

Errors are ordered by the reading order of the line they refer to. Every command except `check`
prints only the first error and exits 1. `check` prints all of them (section 3.8).

A main file that cannot be read prints `error: cannot read journal: FILE` (FILE as given) and
exits 1, for every command.

**Option values** are validated before the journal is read. An invalid value prints one line
and exits 2:

| Option value | Rule | Message |
|---|---|---|
| `--begin D`, `--end D` | a valid `YYYY-MM-DD` date as in 1.1 | `error: invalid date: D` |
| `--depth N` | `[1-9][0-9]*` | `error: invalid depth: N` |
| `--value C` | a COMMODITY, `[A-Z]+` | `error: invalid commodity: C` |
| a PATTERN | compiles as a Python regular expression | `error: invalid pattern: PATTERN` |

(If several values are invalid, which one is reported is unspecified.)

## 3. Commands

**Patterns.** A PATTERN is a regular expression searched (Python `re.search`, case-insensitive)
in the full account name of a posting. With several patterns a posting matches if any pattern
matches; with none, every posting matches.

**Dates.** `--begin D` keeps transactions dated on or after D (inclusive); `--end D` keeps those
dated before D (exclusive).

**Account order.** Wherever accounts are sorted, they are compared as lists of components, each
component by Unicode code point: `Assets` < `Assets:Bank` < `Assets:Bank:Checking` <
`Assets:Cash` < `Assets Fund` < `Expenses`. (This is not plain string order, which would put
`Assets Fund` before `Assets:Bank`.)

### 3.1 `balance [PATTERN...] [--depth N] [--flat] [--empty] [--begin D] [--end D] [--value C]`

Reports account balances from the **matched postings**: postings of transactions in the date
range whose account matches the patterns.

**Tree mode** (default). The tree's nodes are every account with matched postings and all of
its ancestors (`Assets:Bank:Checking` has ancestors `Assets:Bank` and `Assets`), limited by
`--depth N` to nodes with at most N components (deeper accounts count towards their ancestor
at depth N). A node's amount is its **inclusive balance**: the sum of the matched postings of
the node's account and of all its descendant accounts. A node is shown when its inclusive
balance or the inclusive balance of any of its (non-clipped) descendants is non-zero; with
`--empty` every node is shown. Nodes appear in account order (so every node directly follows
its parent or its previous sibling's subtree); a node at depth k (1 for top level) is indented
by 2 × (k − 1) spaces and shows only its last name component.

**Flat mode** (`--flat`). One row per account with matched postings, with the full account name
and no indentation, in account order. The amount is the sum of the matched postings of exactly
that account (not its descendants). With `--depth N`, an account with more than N components is
replaced by its ancestor of N components, and the rows of accounts that become equal are
merged (amounts summed). A row is shown when its amount is non-zero, or always with `--empty`.

**Layout.** Each row's amount is a balance (1.2), possibly several lines. Let W be the length of
the longest amount string in the whole output (rows and total). Every amount string is
right-aligned to W characters. A row with several commodities prints one line per commodity;
the account name appears only on the last of them, after two spaces and the indentation; the
other lines contain only the amount. After the rows comes a line of W `-` characters, then the
total: the balance of all matched postings, right-aligned to W, one line per commodity. With no
shown rows, only the separator and the total are printed. No line has trailing whitespace.

See section 4 for complete examples.

**`--value C`** converts each matched posting amount in another commodity X into C using the
market price of X in C: among the `P` directives for X priced in C (the priced commodity is X
and COMMODITY2 is C) dated on or before the valuation date, the one with the latest date; for
several on that date, the last in reading order. The valuation date is the day before `--end`
when `--end` is given; without `--end` every price directive is eligible. Only direct prices
are used (no inverse prices, no chains, no transaction costs). An amount with no eligible price
is left unconverted. Conversion is exact (quantity × price); rounding happens only on display,
at C's display precision.

### 3.2 `register [PATTERN...] [--begin D] [--end D] [--running]`

Lists the matched postings (as in 3.1), transactions sorted by date (stable, so reading order breaks
ties), postings in transaction order. A posting without an amount shows its inferred amount.
Costs are not shown. Each posting is one line:

| Column | Width | Content |
|---|---|---|
| date | 10 | `YYYY-MM-DD` |
| payee | 20 | the payee |
| account | 24 | the full account name |
| amount | 14 | the formatted posting amount, right-aligned |
| total | 14 | only with `--running`: the running total, right-aligned |

Columns are separated by one space. Payee and account are left-aligned and padded to their
width; text longer than the width is cut to (width − 2) characters followed by `..` (text of
exactly the width is kept). Widths count characters. The date and payee are printed only on the
first listed posting of each transaction; on its other lines both columns are blank (spaces).
Amounts are never cut; a longer one simply makes its line longer.

The running total is the balance (1.2) of all listed postings so far, including the current
one. When it has several commodities the line shows the first (in commodity order) and one
continuation line per further commodity follows: 72 spaces (the blank date, payee, account and
amount columns with their separators), then the commodity's amount right-aligned to 14
characters, whatever the length of the posting's own line. Every line is stripped of trailing
whitespace.

```
2024-01-01 Opening balance      Assets:Bank                2,000.00 USD
                                Equity:Opening            -2,000.00 USD
2024-01-05 Supermarket on the.. Expenses:Food                 45.10 USD
```

### 3.3 `print [PATTERN...] [--begin D] [--end D]`

Prints, in canonical form, every transaction in the date range that has at least one posting
matching the patterns, sorted by date (stable). Transactions are separated by one blank line;
with none, nothing is printed. A transaction is printed as:

- the header `DATE PAYEE`, or `DATE S PAYEE` with S the status;
- its comment lines and postings in their original order. A comment line is four spaces, `; `
  and the comment text (just `    ;` when the text is empty);
- a posting written without an amount is four spaces and the account name (it stays elided);
- a posting with an amount is four spaces, the account name left-aligned to A, two spaces and
  the amount right-aligned to M, then for a cost ` @ ` or ` @@ ` and the formatted cost amount.
  A is the length of the longest account name among the transaction's postings that have an
  amount, M the length of the longest formatted amount (without cost) among them.

Amounts use the display format of 1.2. Inline comments and the original spacing are not
reproduced. Directives are not printed. For example the transaction

```
2024-01-08 * Broker ; note
  Assets:Broker   5 AAPL @ 190.00 USD
\tFees\t1 USD
  Assets:Bank
```

(each `\t` standing for a tab character) is printed as

```
2024-01-08 * Broker
    Assets:Broker    5 AAPL @ 190.00 USD
    Fees           1.00 USD
    Assets:Bank
```

### 3.4 `accounts [PATTERN...]`

Prints, one per line in account order, every distinct account name used in a posting or
declared by an `account` directive and matching the patterns. Ancestors that are never used
or declared themselves are not listed.

### 3.5 `payees`

Prints every distinct payee, one per line, sorted by Unicode code point.

### 3.6 `prices`

Prints every `P` directive as `P DATE COMMODITY AMOUNT`, with AMOUNT formatted as in 1.2 (in
COMMODITY2's display precision), sorted by COMMODITY (code point), then date, then reading
order.

### 3.7 `stats`

Prints exactly these eight lines:

```
Files: 2
Transactions: 6
Postings: 14
Accounts: 7
Payees: 5
Commodities: AAPL, EUR, USD
Date range: 2024-01-01 to 2024-03-31
Prices: 3
```

- `Files`: the number of files read (the main file plus every successful include; a file
  included twice counts twice).
- `Accounts`: distinct accounts used in postings (declarations alone do not count).
- `Commodities`: distinct commodities appearing anywhere (posting amounts, costs, `P`
  directives), sorted, joined by `, `; `none` if there are none.
- `Date range`: the earliest and latest transaction dates; `none` without transactions.
- `Prices`: the number of `P` directives.

### 3.8 `check`

Validates the journal. If it is valid, prints `ok: F files, T transactions, P postings` and
exits 0, with each noun singular when its count is 1 (`ok: 1 file, 1 transaction, 2 postings`).
Otherwise prints every error (section 2) to stderr in order, then a line `N errors` (or
`1 error`) to stderr, and exits 1.

## 4. Worked example

`main.journal`:

```
; Household books
account Assets:Bank
account Assets:Broker
account Equity:Opening
P 2024-01-31 AAPL 190.00 USD
P 2024-02-29 AAPL 181.5 USD
P 2024-02-01 EUR 1.08 USD

2024-01-01 * Opening balance
    Assets:Bank      2,000.00 USD
    Equity:Opening

include 2024-02.journal

2024-01-05 Supermarket on the corner   ; weekly
    ; paid by card
    Expenses:Food    45.1 USD
    Assets:Bank

2024-01-08 ! Broker
    Assets:Broker    5 AAPL @ 180 USD
    Assets:Bank      -900 USD
```

`2024-02.journal` (in the same directory):

```
2024-02-03 * Café de Paris
    Expenses:Eating Out    12.50 EUR
    Assets:Wallet

2024-02-10 Broker
    Assets:Bank      380 USD
    Assets:Broker    -2 AAPL @@ 380 USD  ; sold two
```

Outputs (`$ ` lines are the commands, shown without the `python3 -m ledger -f main.journal`
prefix; stderr is shown for the failing `--strict check`):

```
$ balance
       3 AAPL
   -12.50 EUR
 1,434.90 USD  Assets
 1,434.90 USD    Bank
       3 AAPL    Broker
   -12.50 EUR    Wallet
-2,000.00 USD  Equity
-2,000.00 USD    Opening
    12.50 EUR
    45.10 USD  Expenses
    12.50 EUR    Eating Out
    45.10 USD    Food
-------------
       3 AAPL
  -520.00 USD
```

```
$ balance --flat
 1,434.90 USD  Assets:Bank
       3 AAPL  Assets:Broker
   -12.50 EUR  Assets:Wallet
-2,000.00 USD  Equity:Opening
    12.50 EUR  Expenses:Eating Out
    45.10 USD  Expenses:Food
-------------
       3 AAPL
  -520.00 USD
```

```
$ balance --depth 1 --value USD --end 2024-02-15
 1,991.40 USD  Assets
-2,000.00 USD  Equity
    58.60 USD  Expenses
-------------
    50.00 USD
```

```
$ balance --value USD
 1,965.90 USD  Assets
 1,434.90 USD    Bank
   544.50 USD    Broker
   -13.50 USD    Wallet
-2,000.00 USD  Equity
-2,000.00 USD    Opening
    58.60 USD  Expenses
    13.50 USD    Eating Out
    45.10 USD    Food
-------------
    24.50 USD
```

```
$ balance assets --begin 2024-02-01
   -2 AAPL
-12.50 EUR
380.00 USD  Assets
380.00 USD    Bank
   -2 AAPL    Broker
-12.50 EUR    Wallet
----------
   -2 AAPL
-12.50 EUR
380.00 USD
```

```
$ register --running
2024-01-01 Opening balance      Assets:Bank                2,000.00 USD   2,000.00 USD
                                Equity:Opening            -2,000.00 USD              0
2024-01-05 Supermarket on the.. Expenses:Food                 45.10 USD      45.10 USD
                                Assets:Bank                  -45.10 USD              0
2024-01-08 Broker               Assets:Broker                    5 AAPL         5 AAPL
                                Assets:Bank                 -900.00 USD         5 AAPL
                                                                           -900.00 USD
2024-02-03 Café de Paris        Expenses:Eating Out           12.50 EUR         5 AAPL
                                                                             12.50 EUR
                                                                           -900.00 USD
                                Assets:Wallet                -12.50 EUR         5 AAPL
                                                                           -900.00 USD
2024-02-10 Broker               Assets:Bank                  380.00 USD         5 AAPL
                                                                           -520.00 USD
                                Assets:Broker                   -2 AAPL         3 AAPL
                                                                           -520.00 USD
```

```
$ register bank
2024-01-01 Opening balance      Assets:Bank                2,000.00 USD
2024-01-05 Supermarket on the.. Assets:Bank                  -45.10 USD
2024-01-08 Broker               Assets:Bank                 -900.00 USD
2024-02-10 Broker               Assets:Bank                  380.00 USD
```

```
$ print
2024-01-01 * Opening balance
    Assets:Bank  2,000.00 USD
    Equity:Opening

2024-01-05 Supermarket on the corner
    ; paid by card
    Expenses:Food  45.10 USD
    Assets:Bank

2024-01-08 ! Broker
    Assets:Broker       5 AAPL @ 180.00 USD
    Assets:Bank    -900.00 USD

2024-02-03 * Café de Paris
    Expenses:Eating Out  12.50 EUR
    Assets:Wallet

2024-02-10 Broker
    Assets:Bank    380.00 USD
    Assets:Broker     -2 AAPL @@ 380.00 USD
```

```
$ accounts
Assets:Bank
Assets:Broker
Assets:Wallet
Equity:Opening
Expenses:Eating Out
Expenses:Food
```

```
$ payees
Broker
Café de Paris
Opening balance
Supermarket on the corner
```

```
$ prices
P 2024-01-31 AAPL 190.00 USD
P 2024-02-29 AAPL 181.50 USD
P 2024-02-01 EUR 1.08 USD
```

```
$ stats
Files: 2
Transactions: 5
Postings: 10
Accounts: 6
Payees: 4
Commodities: AAPL, EUR, USD
Date range: 2024-01-01 to 2024-02-10
Prices: 3
```

```
$ check
ok: 2 files, 5 transactions, 10 postings
```

```
$ --strict check
error: 2024-02.journal:2: undeclared account: Expenses:Eating Out
error: 2024-02.journal:3: undeclared account: Assets:Wallet
error: main.journal:17: undeclared account: Expenses:Food
3 errors
```

## 5. Done means

- Every command behaves exactly as specified, output character for character.
- `tests/` contains a unittest suite (at least 20 tests, all passing with
  `python3 -m unittest discover -s tests`) covering parsing, balancing, includes, every
  command and the error messages.
- `README.md` has a `## Usage` section with a `python3 -m ledger -f FILE COMMAND ...` example of
  every command (`balance`, `register`, `print`, `accounts`, `payees`, `prices`, `stats`,
  `check`).
