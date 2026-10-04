import contextlib
import io
import tempfile
import textwrap
import unittest
from pathlib import Path

from ledger.cli import main

BOOKS = """\
account Assets:Bank
P 2024-01-31 AAPL 190.00 USD

2024-01-01 * Opening balance
    Assets:Bank      2,000.00 USD
    Equity:Opening

2024-01-05 Supermarket on the corner  ; weekly
    ; paid by card
    Expenses:Food    45.1 USD
    Assets:Bank

2024-01-08 ! Broker
    Assets:Broker    5 AAPL @ 180 USD
    Assets:Bank      -900 USD
"""


class LedgerTestCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def write(self, name: str, text: str) -> str:
        path = self.dir / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(text), encoding="utf-8")
        return str(path)

    def run_cli(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = main(list(args))
            except SystemExit as exit_:
                code = exit_.code
        return code, out.getvalue(), err.getvalue()

    def ok(self, journal: str, *args) -> str:
        code, out, err = self.run_cli("-f", journal, *args)
        self.assertEqual((code, err), (0, ""))
        return out

    def error(self, journal: str, *args) -> tuple[int, str]:
        code, out, err = self.run_cli("-f", journal, *args)
        self.assertEqual(out, "")
        return code, err

    def books(self) -> str:
        return self.write("books.journal", BOOKS)


class ParsingTest(LedgerTestCase):
    def test_inferred_amount_and_thousands(self):
        out = self.ok(self.books(), "register", "food")
        self.assertEqual(out, "2024-01-05 Supermarket on the.. Expenses:Food                 45.10 USD\n")

    def test_unbalanced_transaction_reports_exact_residual(self):
        path = self.write("j", "2024-01-01 x\n  A  10.00 USD\n  B  -9.990 USD\n")
        self.assertEqual(self.error(path, "balance"),
                         (1, f"error: {path}:1: transaction does not balance: residual 0.010 USD\n"))

    def test_cost_residual_uses_product_decimals(self):
        path = self.write("j", "2024-01-01 x\n  A  3 X @ 1.10 USD\n  B  -3.2 USD\n")
        self.assertIn("residual 0.10 USD", self.error(path, "balance")[1])

    def test_multiple_commodities_need_costs(self):
        path = self.write("j", "2024-01-01 x\n  A  1 USD\n  B  1 EUR\n  C\n")
        self.assertIn("cannot infer amount: residual in multiple commodities", self.error(path, "check")[1])

    def test_invalid_numbers(self):
        for number in ["1,23", "1234,567", ".5", "5.", "+5"]:
            with self.subTest(number=number):
                path = self.write("j", f"2024-01-01 x\n  A  {number} USD\n  B\n")
                self.assertEqual(self.error(path, "stats"), (1, f"error: {path}:2: invalid amount: {number} USD\n"))

    def test_invalid_date_and_missing_payee(self):
        path = self.write("j", "2024-02-30 x\n  A  1 USD\n  B\n\n2024-03-01 *\n  A  1 USD\n  B\n")
        code, err = self.error(path, "check")
        self.assertEqual(code, 1)
        self.assertEqual(err, f"error: {path}:1: invalid date: 2024-02-30\nerror: {path}:5: missing payee\n2 errors\n")

    def test_unexpected_indented_line(self):
        path = self.write("j", "; comment\n  A  1 USD\naccount A\n  B\n")
        _, err = self.error(path, "check")
        self.assertEqual(err, f"error: {path}:2: unexpected indented line\n"
                              f"error: {path}:4: unexpected indented line\n2 errors\n")

    def test_unknown_directive(self):
        path = self.write("j", "commodity USD\n")
        self.assertEqual(self.error(path, "accounts")[1], f"error: {path}:1: unknown directive: commodity\n")


class IncludeTest(LedgerTestCase):
    def test_include_relative_to_including_file(self):
        main_path = self.write("books/main.journal", "include sub/a.journal\n")
        self.write("books/sub/a.journal", "2024-01-01 x\n  A  1 USD\n  B\n")
        self.assertEqual(self.ok(main_path, "check"), "ok: 2 files, 1 transaction, 2 postings\n")

    def test_include_cycle_and_display_names(self):
        main_path = self.write("books/main.journal", "include sub/a.journal\n")
        self.write("books/sub/a.journal", "include ../main.journal\n")
        _, err = self.error(main_path, "check")
        self.assertEqual(err, f"error: {self.dir}/books/sub/a.journal:1: include cycle: ../main.journal\n1 error\n")

    def test_missing_include(self):
        path = self.write("j", "include nope.journal\n")
        self.assertIn("cannot read include: nope.journal", self.error(path, "stats")[1])

    def test_missing_main_file(self):
        missing = str(self.dir / "missing.journal")
        self.assertEqual(self.error(missing, "stats"), (1, f"error: cannot read journal: {missing}\n"))


class BalanceTest(LedgerTestCase):
    def test_tree(self):
        self.assertEqual(self.ok(self.books(), "balance"), (
            "       5 AAPL\n"
            " 1,054.90 USD  Assets\n"
            " 1,054.90 USD    Bank\n"
            "       5 AAPL    Broker\n"
            "-2,000.00 USD  Equity\n"
            "-2,000.00 USD    Opening\n"
            "    45.10 USD  Expenses\n"
            "    45.10 USD    Food\n"
            "-------------\n"
            "       5 AAPL\n"
            "  -900.00 USD\n"))

    def test_flat_with_depth(self):
        out = self.ok(self.books(), "balance", "--flat", "--depth", "1")
        self.assertEqual(out, "       5 AAPL\n 1,054.90 USD  Assets\n-2,000.00 USD  Equity\n"
                              "    45.10 USD  Expenses\n-------------\n       5 AAPL\n  -900.00 USD\n")

    def test_value_uses_latest_price_before_end(self):
        out = self.ok(self.books(), "balance", "broker", "--value", "USD")
        self.assertEqual(out, "950.00 USD  Assets\n950.00 USD    Broker\n----------\n950.00 USD\n")
        out = self.ok(self.books(), "balance", "broker", "--value", "USD", "--end", "2024-01-31")
        self.assertEqual(out, "5 AAPL  Assets\n5 AAPL    Broker\n------\n5 AAPL\n")

    def test_date_range_and_no_rows(self):
        self.assertEqual(self.ok(self.books(), "balance", "--begin", "2025-01-01"), "-\n0\n")

    def test_zero_parent_with_nonzero_children_is_shown(self):
        path = self.write("j", "2024-01-01 x\n  A:B  1 USD\n  A:C  -1 USD\n")
        self.assertEqual(self.ok(path, "balance"), "     0  A\n 1 USD    B\n-1 USD    C\n------\n     0\n")

    def test_invalid_option_values(self):
        books = self.books()
        self.assertEqual(self.run_cli("-f", books, "balance", "--depth", "0")[2], "error: invalid depth: 0\n")
        self.assertEqual(self.run_cli("-f", books, "balance", "--value", "usd")[0], 2)
        self.assertEqual(self.run_cli("-f", books, "register", "--end", "2024-13-01")[2],
                         "error: invalid date: 2024-13-01\n")
        self.assertEqual(self.run_cli("-f", books, "print", "(")[2], "error: invalid pattern: (\n")


class RegisterTest(LedgerTestCase):
    def test_running_total_with_several_commodities(self):
        out = self.ok(self.books(), "register", "--running", "--begin", "2024-01-08")
        self.assertEqual(out,
                         "2024-01-08 Broker               Assets:Broker                    5 AAPL         5 AAPL\n"
                         "                                Assets:Bank                 -900.00 USD         5 AAPL\n"
                         + " " * 72 + "   -900.00 USD\n")

    def test_truncation(self):
        path = self.write("j", "2024-01-01 Twenty characters ok\n  Assets:Exactly:24:Chars:X  1 USD\n  B\n")
        first = self.ok(path, "register").splitlines()[0]
        self.assertEqual(first, "2024-01-01 Twenty characters ok Assets:Exactly:24:Char..          1 USD")


class OtherCommandsTest(LedgerTestCase):
    def test_print_is_canonical(self):
        self.assertEqual(self.ok(self.books(), "print", "broker"),
                         "2024-01-08 ! Broker\n    Assets:Broker       5 AAPL @ 180.00 USD\n"
                         "    Assets:Bank    -900.00 USD\n")

    def test_print_keeps_comments_and_elided_postings(self):
        self.assertEqual(self.ok(self.books(), "print", "food"),
                         "2024-01-05 Supermarket on the corner\n    ; paid by card\n"
                         "    Expenses:Food  45.10 USD\n    Assets:Bank\n")

    def test_accounts_use_component_order(self):
        path = self.write("j", "account Assets Fund\naccount Assets:Bank\naccount Assets\n")
        self.assertEqual(self.ok(path, "accounts"), "Assets\nAssets:Bank\nAssets Fund\n")

    def test_payees_and_prices(self):
        self.assertEqual(self.ok(self.books(), "payees"), "Broker\nOpening balance\nSupermarket on the corner\n")
        self.assertEqual(self.ok(self.books(), "prices"), "P 2024-01-31 AAPL 190.00 USD\n")

    def test_stats(self):
        self.assertEqual(self.ok(self.books(), "stats"), textwrap.dedent("""\
            Files: 1
            Transactions: 3
            Postings: 6
            Accounts: 4
            Payees: 3
            Commodities: AAPL, USD
            Date range: 2024-01-01 to 2024-01-08
            Prices: 1
            """))

    def test_strict_mode(self):
        books = self.books()
        self.assertEqual(self.ok(books, "check"), "ok: 1 file, 3 transactions, 6 postings\n")
        code, err = self.error(books, "--strict", "check")
        self.assertEqual(code, 1)
        self.assertEqual(err.splitlines()[0], f"error: {books}:6: undeclared account: Equity:Opening")
        self.assertEqual(err.splitlines()[-1], "3 errors")


if __name__ == "__main__":
    unittest.main()
