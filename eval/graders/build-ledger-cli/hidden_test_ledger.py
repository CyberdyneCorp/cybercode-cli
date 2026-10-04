"""Hidden behavior tests for build-ledger-cli. Drives `python3 -m ledger` via subprocess.

The package under test is found through LEDGER_WORKSPACE; the frozen reference implementation
through LEDGER_REFERENCE. Two parts:
  * explicit cases (ledger_cases.py): fixed journals and argv with the exact expected exit code,
    stdout and stderr, each its own test;
  * randomized differential tests: seeded random journals (valid and invalid, with includes,
    costs, prices, comments, Unicode) run through every command, compared with the reference.
"""

import contextlib
import io
import os
import random
import subprocess
import sys
import tempfile
import unittest
from decimal import Decimal
from pathlib import Path

from ledger_cases import ARGPARSE_CASES, CASES

WORKSPACE = os.environ["LEDGER_WORKSPACE"]
sys.path.insert(0, os.environ["LEDGER_REFERENCE"])
from ledger.cli import main as reference_main  # noqa: E402  (the frozen reference, not the agent's code)

TIMEOUT = 30
RANDOM_JOURNALS = 24


def write_files(root: Path, files: dict[str, str]) -> None:
    for name, text in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode("utf-8"))


def run_agent(cwd: Path, args: list[str]) -> tuple[int, str, str]:
    env = dict(os.environ, PYTHONPATH=WORKSPACE, PYTHONDONTWRITEBYTECODE="1", PYTHONIOENCODING="utf-8",
               PYTHONUTF8="1")
    try:
        result = subprocess.run([sys.executable, "-m", "ledger", *args], cwd=cwd, env=env,
                                capture_output=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        return -1, "", f"timed out after {TIMEOUT}s"
    return (result.returncode, result.stdout.decode("utf-8", "replace"),
            result.stderr.decode("utf-8", "replace"))


def run_reference(cwd: Path, args: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    previous = os.getcwd()
    os.chdir(cwd)
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = reference_main(list(args))
            except SystemExit as exit_:
                code = exit_.code
    finally:
        os.chdir(previous)
    return code, out.getvalue(), err.getvalue()


def describe(args, expected, actual) -> str:
    labels = ("exit code", "stdout", "stderr")
    for label, want, got in zip(labels, expected, actual):
        if want != got:
            return (f"`ledger {' '.join(args)}`: {label} differs\n--- expected\n{want}\n--- actual\n{got}")
    return ""


class ExplicitCases(unittest.TestCase):
    """One generated test method per entry of ledger_cases.CASES."""


def make_case_test(case: dict):
    def test(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_files(Path(tmp), case["files"])
            actual = run_agent(Path(tmp), case["args"])
        expected = (case["code"], case["stdout"], case["stderr"])
        if actual != expected:
            self.fail(describe(case["args"], expected, actual))
    return test


for _case in CASES:
    setattr(ExplicitCases, f"test_{_case['name']}", make_case_test(_case))


class ArgparseUsageErrors(unittest.TestCase):
    """Usage errors reported by argparse: exit code 2 and nothing on stdout."""


def make_usage_test(args: list[str]):
    def test(self):
        with tempfile.TemporaryDirectory() as tmp:
            write_files(Path(tmp), {"main.journal": "2024-01-01 x\n  A  1 USD\n  B\n"})
            code, out, _ = run_agent(Path(tmp), args)
        self.assertEqual((code, out), (2, ""), f"`ledger {' '.join(args)}`")
    return test


for _name, _args in ARGPARSE_CASES:
    setattr(ArgparseUsageErrors, f"test_{_name}", make_usage_test(_args))


# -- random journals -------------------------------------------------------------------------

ACCOUNTS = ["Assets", "Assets:Bank", "Assets:Bank:Checking", "Assets:Bank:Savings", "Assets:Cash",
            "Assets Fund", "Expenses:Food", "Expenses:Food:Snacks", "Expenses:Eating Out",
            "Expenses:A rather long account name", "Income:Salary", "Liabilities:Card", "Equity:Opening",
            "Ünicode:Konto", "assets:lower"]
PAYEES = ["Shop", "Grocery store on Main Street", "Café", "Employer Inc", "x", "Landlord",
          "Ünïcödé payee with a long name", "Exactly twenty chars"]
COMMODITIES = ["USD", "EUR", "BTC"]
SEPARATORS = ["  ", "   ", "\t", "      ", "  \t"]


class JournalGenerator:
    def __init__(self, seed: int):
        self.rng = random.Random(seed)

    def date(self) -> str:
        return f"2024-{self.rng.randint(1, 6):02d}-{self.rng.randint(1, 28):02d}"

    def quantity(self) -> Decimal:
        places = self.rng.choice([0, 0, 1, 2, 2, 3])
        magnitude = self.rng.choice([10, 100, 1000, 100000, 10000000])
        value = Decimal(self.rng.randint(1, magnitude)).scaleb(-places)
        return value if self.rng.random() < 0.7 else -value

    def number(self, value: Decimal) -> str:
        text = f"{value:f}"
        if abs(value) >= 1000 and self.rng.random() < 0.5:
            text = f"{value:,f}"
        return text

    def posting(self, account: str, amount: str | None) -> str:
        indent = self.rng.choice(["  ", "    ", "\t"])
        line = indent + account
        if amount is not None:
            line += self.rng.choice(SEPARATORS) + amount
        if self.rng.random() < 0.1:
            line += "  ; note " + self.rng.choice(["1 USD", "x", ""])
        return line

    def transaction(self) -> list[str]:
        rng = self.rng
        status = rng.choice(["", "", "* ", "! "])
        lines = [f"{self.date()} {status}{rng.choice(PAYEES)}" + ("  ; hdr" if rng.random() < 0.1 else "")]
        if rng.random() < 0.15:
            lines.append("    ; " + rng.choice(["memo", "", "Ünïcode memo"]))
        accounts = rng.sample(ACCOUNTS, rng.randint(2, 4))
        if rng.random() < 0.25:
            quantity = abs(self.quantity())
            price = Decimal(rng.randint(1, 50000)).scaleb(-rng.choice([0, 2]))
            commodity = rng.choice(["AAPL", "BTC"])
            sign = rng.choice([1, -1])
            if rng.random() < 0.5:
                lines.append(self.posting(accounts[0], f"{self.number(sign * quantity)} {commodity} @ {price} USD"))
                weight = sign * quantity * price
            else:
                total = (quantity * price).quantize(Decimal("0.01"))
                lines.append(self.posting(accounts[0], f"{self.number(sign * quantity)} {commodity} @@ {total} USD"))
                weight = sign * total
            other = None if rng.random() < 0.5 else f"{self.number(-weight)} USD"
            lines.append(self.posting(accounts[1], other))
            return lines
        commodity = rng.choice(COMMODITIES)
        total = Decimal(0)
        for account in accounts[:-1]:
            quantity = self.quantity()
            total += quantity
            lines.append(self.posting(account, f"{self.number(quantity)} {commodity}"))
        if total == 0 or rng.random() < 0.5:
            last = None if total != 0 else f"0 {commodity}"
        else:
            last = f"{self.number(-total)} {commodity}"
        lines.append(self.posting(accounts[-1], last))
        return lines

    def directive(self) -> str:
        rng = self.rng
        if rng.random() < 0.5:
            return "account " + rng.choice(ACCOUNTS)
        commodity, target = rng.choice([("AAPL", "USD"), ("BTC", "USD"), ("EUR", "USD"), ("USD", "EUR"), ("BTC", "EUR")])
        price = Decimal(rng.randint(1, 900000)).scaleb(-rng.choice([0, 1, 2, 3, 4]))
        return f"P {self.date()} {commodity} {price} {target}"

    def corrupt(self, entry: list[str]) -> list[str]:
        rng = self.rng
        kind = rng.randrange(8)
        entry = list(entry)
        if kind == 0:
            entry[0] = "2024-02-30" + entry[0][10:]
        elif kind == 1:
            entry[0] = entry[0][:10]
        elif kind == 2:
            entry.append("  Broken::Account  1 USD")
        elif kind == 3:
            entry.append("  Extra  1,23 USD")
        elif kind == 4:
            entry.append("  Unbalancing  0.001 USD")
        elif kind == 5:
            entry = ["frobnicate now"] + entry
        elif kind == 6:
            entry = entry[:1]
        else:
            entry = ["  orphan  1 USD"]
        return entry

    def entries(self, count: int, invalid: bool) -> list[list[str]]:
        result = []
        for _ in range(count):
            entry = self.transaction() if self.rng.random() < 0.75 else [self.directive()]
            if invalid and self.rng.random() < 0.15:
                entry = self.corrupt(entry)
            result.append(entry)
        return result

    def render(self, entries: list[list[str]]) -> str:
        chunks = []
        for entry in entries:
            if self.rng.random() < 0.1:
                chunks.append(self.rng.choice(["; a comment", "# another"]))
            chunks.append("\n".join(entry))
            chunks.append("" if self.rng.random() < 0.85 else "   ")
        return "\n".join(chunks) + "\n"

    def journal(self) -> dict[str, str]:
        invalid = self.rng.random() < 0.35
        main = self.entries(self.rng.randint(4, 14), invalid)
        sub = self.entries(self.rng.randint(0, 6), invalid)
        deeper = self.entries(self.rng.randint(0, 3), invalid)
        files = {"sub/part.journal": self.render(sub) + "include ../deeper.journal\n",
                 "deeper.journal": self.render(deeper)}
        position = self.rng.randint(0, len(main))
        main.insert(position, ["include sub/part.journal"])
        files["main.journal"] = self.render(main)
        return files

    def commands(self) -> list[list[str]]:
        rng = self.rng
        pattern = rng.choice(["bank", "^exp", "food|cash", "Ü", "ASSETS", ":s", "salary$"])
        begin, end = sorted([self.date(), self.date()])
        f = ["-f", "main.journal"]
        return [
            f + ["balance"],
            f + ["balance", "--flat"],
            f + ["balance", "--depth", str(rng.randint(1, 3)), "--empty"],
            f + ["balance", pattern, "--begin", begin, "--end", end],
            f + ["balance", "--value", "USD", "--end", end],
            f + ["balance", "--flat", "--depth", "1", "--value", rng.choice(["EUR", "USD"])],
            f + ["register", "--running"],
            f + ["register", pattern, "--begin", begin],
            f + ["print"],
            f + ["print", pattern, "--end", end],
            f + ["accounts", rng.choice(["", "a"])] if rng.random() < 0.5 else f + ["accounts"],
            f + ["payees"],
            f + ["prices"],
            f + ["stats"],
            f + ["check"],
            f + ["--strict", "check"],
        ]


class RandomDifferential(unittest.TestCase):
    """Seeded random journals; every command must match the frozen reference exactly."""


def make_random_test(seed: int):
    def test(self):
        generator = JournalGenerator(seed)
        files = generator.journal()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_files(root, files)
            for args in generator.commands():
                args = [a for a in args if a != ""]
                expected = run_reference(root, args)
                actual = run_agent(root, args)
                if actual != expected:
                    journal = "\n".join(f"=== {name}\n{text}" for name, text in sorted(files.items()))
                    self.fail(describe(args, expected, actual) + f"\n--- journal files\n{journal}")
    return test


for _seed in range(RANDOM_JOURNALS):
    setattr(RandomDifferential, f"test_random_journal_{_seed:02d}", make_random_test(1000 + _seed))


if __name__ == "__main__":
    unittest.main()
