"""Report rendering. Each function returns the output lines of one command."""

import datetime
import re
from dataclasses import dataclass

from ledger.amounts import Amount, Balance, Formatter
from ledger.journal import Comment, Journal, Posting, Transaction


@dataclass
class Filters:
    patterns: list[re.Pattern]
    begin: datetime.date | None = None
    end: datetime.date | None = None

    def account(self, name: str) -> bool:
        return not self.patterns or any(p.search(name) for p in self.patterns)

    def date(self, date: datetime.date) -> bool:
        return (self.begin is None or date >= self.begin) and (self.end is None or date < self.end)

    def transactions(self, journal: Journal) -> list[Transaction]:
        return [t for t in journal.sorted_transactions() if self.date(t.date)]

    def postings(self, journal: Journal) -> list[tuple[Transaction, Posting]]:
        return [(t, p) for t in self.transactions(journal) for p in t.postings if self.account(p.account)]


def account_key(name: str) -> list[str]:
    return name.split(":")


def plural(count: int, noun: str) -> str:
    return f"{count} {noun}" if count == 1 else f"{count} {noun}s"


# -- balance -------------------------------------------------------------------------------

class Valuer:
    """Converts amounts into a target commodity with the latest eligible market price."""

    def __init__(self, journal: Journal, target: str, valuation_date: datetime.date | None):
        self.target = target
        self.rates = {}
        for price in sorted(journal.prices, key=lambda p: (p.date, p.order)):
            if price.price.commodity == target and (valuation_date is None or price.date <= valuation_date):
                self.rates[price.commodity] = price.price.quantity

    def convert(self, amount: Amount) -> Amount:
        rate = self.rates.get(amount.commodity)
        return amount if rate is None else Amount(amount.quantity * rate, self.target)


def balance(journal: Journal, filters: Filters, depth: int | None, flat: bool, empty: bool,
            value: str | None) -> list[str]:
    valuer = None
    if value:
        valuation_date = filters.end - datetime.timedelta(days=1) if filters.end else None
        valuer = Valuer(journal, value, valuation_date)
    own: dict[str, Balance] = {}
    total = Balance()
    for _, posting in filters.postings(journal):
        amount = valuer.convert(posting.amount) if valuer else posting.amount
        own.setdefault(posting.account, Balance()).add(amount)
        total.add(amount)
    rows = flat_rows(own, depth, empty) if flat else tree_rows(own, depth, empty)
    return render_balance(rows, total, Formatter(journal.precision))


def clip(name: str, depth: int | None) -> str:
    return ":".join(account_key(name)[:depth]) if depth else name


def flat_rows(own: dict[str, Balance], depth: int | None, empty: bool) -> list[tuple[str, Balance]]:
    merged: dict[str, Balance] = {}
    for account, amount in own.items():
        merged.setdefault(clip(account, depth), Balance()).add_balance(amount)
    return [(name, merged[name]) for name in sorted(merged, key=account_key)
            if empty or not merged[name].is_zero()]


def tree_rows(own: dict[str, Balance], depth: int | None, empty: bool) -> list[tuple[str, Balance]]:
    inclusive: dict[tuple[str, ...], Balance] = {}
    for account, amount in own.items():
        parts = tuple(account_key(account))
        for size in range(1, min(len(parts), depth or len(parts)) + 1):
            inclusive.setdefault(parts[:size], Balance()).add_balance(amount)
    nonzero = [node for node, amount in inclusive.items() if not amount.is_zero()]
    rows = []
    for node in sorted(inclusive):
        if empty or any(other[: len(node)] == node for other in nonzero):
            rows.append(("  " * (len(node) - 1) + node[-1], inclusive[node]))
    return rows


def render_balance(rows: list[tuple[str, Balance]], total: Balance, fmt: Formatter) -> list[str]:
    rendered = [(label, fmt.balance(amount)) for label, amount in rows]
    total_lines = fmt.balance(total)
    width = max(len(text) for _, texts in rendered + [("", total_lines)] for text in texts)
    lines = []
    for label, texts in rendered:
        lines += [text.rjust(width) for text in texts[:-1]]
        lines.append(f"{texts[-1].rjust(width)}  {label}")
    lines.append("-" * width)
    lines += [text.rjust(width) for text in total_lines]
    return lines


# -- register ------------------------------------------------------------------------------

def fit(text: str, width: int) -> str:
    return text.ljust(width) if len(text) <= width else text[: width - 2] + ".."


def register(journal: Journal, filters: Filters, running: bool) -> list[str]:
    fmt = Formatter(journal.precision)
    total = Balance()
    lines = []
    previous = None
    for transaction, posting in filters.postings(journal):
        first = transaction is not previous
        previous = transaction
        date = str(transaction.date) if first else ""
        payee = transaction.payee if first else ""
        line = f"{date:10} {fit(payee, 20)} {fit(posting.account, 24)} {fmt.amount(posting.amount):>14}"
        if not running:
            lines.append(line.rstrip())
            continue
        total.add(posting.amount)
        totals = fmt.balance(total)
        lines.append(f"{line} {totals[0]:>14}".rstrip())
        lines += [f"{'':{10 + 1 + 20 + 1 + 24 + 1 + 14}} {text:>14}" for text in totals[1:]]
    return lines


# -- print ---------------------------------------------------------------------------------

def print_journal(journal: Journal, filters: Filters) -> list[str]:
    fmt = Formatter(journal.precision)
    blocks = []
    for transaction in filters.transactions(journal):
        if any(filters.account(p.account) for p in transaction.postings):
            blocks.append(print_transaction(transaction, fmt))
    return [line for i, block in enumerate(blocks) for line in ([""] if i else []) + block]


def print_transaction(transaction: Transaction, fmt: Formatter) -> list[str]:
    status = f" {transaction.status}" if transaction.status else ""
    lines = [f"{transaction.date}{status} {transaction.payee}"]
    explicit = [p for p in transaction.postings if not p.elided]
    account_width = max((len(p.account) for p in explicit), default=0)
    amount_width = max((len(fmt.amount(p.amount)) for p in explicit), default=0)
    for item in transaction.items:
        if isinstance(item, Comment):
            lines.append(f"    ; {item.text}".rstrip())
        elif item.elided:
            lines.append(f"    {item.account}")
        else:
            line = f"    {item.account:{account_width}}  {fmt.amount(item.amount):>{amount_width}}"
            if item.cost:
                line += f" {'@@' if item.cost.total else '@'} {fmt.amount(item.cost.amount)}"
            lines.append(line)
    return lines


# -- listings ------------------------------------------------------------------------------

def accounts(journal: Journal, filters: Filters) -> list[str]:
    names = journal.accounts_used() | journal.declared
    return sorted((n for n in names if filters.account(n)), key=account_key)


def payees(journal: Journal) -> list[str]:
    return sorted({t.payee for t in journal.transactions})


def prices(journal: Journal) -> list[str]:
    fmt = Formatter(journal.precision)
    ordered = sorted(journal.prices, key=lambda p: (p.commodity, p.date, p.order))
    return [f"P {p.date} {p.commodity} {fmt.amount(p.price)}" for p in ordered]


def stats(journal: Journal) -> list[str]:
    transactions = journal.sorted_transactions()
    commodities = sorted(journal.precision)
    date_range = f"{transactions[0].date} to {transactions[-1].date}" if transactions else "none"
    return [
        f"Files: {journal.files}",
        f"Transactions: {len(transactions)}",
        f"Postings: {sum(len(t.postings) for t in transactions)}",
        f"Accounts: {len(journal.accounts_used())}",
        f"Payees: {len(payees(journal))}",
        f"Commodities: {', '.join(commodities) or 'none'}",
        f"Date range: {date_range}",
        f"Prices: {len(journal.prices)}",
    ]


def check(journal: Journal) -> list[str]:
    postings = sum(len(t.postings) for t in journal.transactions)
    return [f"ok: {plural(journal.files, 'file')}, {plural(len(journal.transactions), 'transaction')}, "
            f"{plural(postings, 'posting')}"]
