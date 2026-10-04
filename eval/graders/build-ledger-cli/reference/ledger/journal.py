"""Journal parsing: transactions, directives, includes, balancing and validation."""

import datetime
import os
import re
from dataclasses import dataclass, field

from ledger.amounts import Amount, Balance, is_commodity, parse_number

DATE_RE = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")
POSTING_SEPARATOR_RE = re.compile(r" {2,}|\t")


class JournalUnreadable(Exception):
    """The main journal file cannot be read."""


@dataclass(frozen=True)
class Location:
    file: str
    line: int
    order: int  # global reading order of the line


@dataclass
class JournalError:
    location: Location
    message: str

    def __str__(self) -> str:
        return f"error: {self.location.file}:{self.location.line}: {self.message}"


class EntryError(Exception):
    """An error that discards the current transaction or directive."""

    def __init__(self, message: str):
        super().__init__(message)
        self.message = message


@dataclass
class Cost:
    total: bool  # True for `@@ TOTAL`, False for `@ PRICE`
    amount: Amount


@dataclass
class Posting:
    account: str
    amount: Amount | None
    cost: Cost | None
    location: Location
    elided: bool = False

    def weight(self) -> Amount:
        quantity = self.amount.quantity
        if self.cost is None:
            return self.amount
        price = self.cost.amount
        if not self.cost.total:
            return Amount(quantity * price.quantity, price.commodity)
        sign = (quantity > 0) - (quantity < 0)
        return Amount(sign * price.quantity, price.commodity)


@dataclass
class Comment:
    text: str


@dataclass
class Transaction:
    date: datetime.date
    status: str
    payee: str
    location: Location
    items: list = field(default_factory=list)  # Posting and Comment, in file order

    @property
    def postings(self) -> list[Posting]:
        return [item for item in self.items if isinstance(item, Posting)]


@dataclass
class Price:
    date: datetime.date
    commodity: str
    price: Amount
    order: int


@dataclass
class Journal:
    transactions: list[Transaction] = field(default_factory=list)
    prices: list[Price] = field(default_factory=list)
    declared: set[str] = field(default_factory=set)
    precision: dict[str, int] = field(default_factory=dict)
    errors: list[JournalError] = field(default_factory=list)
    files: int = 0

    def sorted_transactions(self) -> list[Transaction]:
        return sorted(self.transactions, key=lambda t: (t.date, t.location.order))

    def accounts_used(self) -> set[str]:
        return {p.account for t in self.transactions for p in t.postings}


def parse_date(token: str) -> datetime.date | None:
    if not DATE_RE.fullmatch(token):
        return None
    try:
        return datetime.date.fromisoformat(token)
    except ValueError:
        return None


def valid_account(name: str) -> bool:
    return all(part and part == part.strip(" ") for part in name.split(":"))


def strip_comment(text: str) -> str:
    return text.split(";", 1)[0].rstrip()


class Parser:
    def __init__(self, strict: bool):
        self.strict = strict
        self.journal = Journal()
        self.order = 0

    # -- files ---------------------------------------------------------------------------

    def parse(self, path: str) -> Journal:
        try:
            text = read_text(path)
        except OSError as exc:
            raise JournalUnreadable(path) from exc
        self.read_lines(text, path, [os.path.realpath(path)])
        if self.strict:
            self.check_declared()
        self.journal.errors.sort(key=lambda e: e.location.order)
        return self.journal

    def read_lines(self, text: str, display: str, chain: list[str]) -> None:
        self.journal.files += 1
        entry: Transaction | None = None
        broken = False  # inside a transaction entry whose transaction is discarded
        for number, raw in enumerate(text.split("\n"), 1):
            line = raw[:-1] if raw.endswith("\r") else raw
            self.order += 1
            location = Location(display, number, self.order)
            if not line.strip(" \t"):
                self.finish(entry)
                entry, broken = None, False
            elif line[0] in " \t":
                if entry is not None:
                    entry, broken = self.guarded(location, self.indented_line, entry, line, location)
                elif not broken:
                    self.error(location, "unexpected indented line")
            else:
                self.finish(entry)
                entry, broken = self.column_zero_line(line, location, display, chain)
        self.finish(entry)

    def column_zero_line(self, line: str, location: Location, display: str, chain: list[str]):
        """Handle a comment, transaction header or directive; return the new (entry, broken) state."""
        if line[0] in ";#":
            return None, False
        if "0" <= line[0] <= "9":
            return self.guarded(location, self.header, line, location)
        self.guarded(location, self.directive, line, location, display, chain)
        return None, False

    def guarded(self, location: Location, method, *args):
        """Run a line handler; on EntryError record it and mark the entry as broken."""
        try:
            return method(*args), False
        except EntryError as exc:
            self.error(location, exc.message)
            return None, True

    def error(self, location: Location, message: str) -> None:
        self.journal.errors.append(JournalError(location, message))

    # -- transactions ------------------------------------------------------------------

    def header(self, line: str, location: Location) -> Transaction:
        text = strip_comment(line)
        token = text.split(None, 1)[0]
        rest = text[len(token):]
        date = parse_date(token)
        if date is None:
            raise EntryError(f"invalid date: {token}")
        rest = rest.strip()
        status = ""
        if rest[:1] in ("*", "!") and (len(rest) == 1 or rest[1] in " \t"):
            status, rest = rest[0], rest[1:].strip()
        if not rest:
            raise EntryError("missing payee")
        return Transaction(date, status, rest, location)

    def indented_line(self, entry: Transaction, line: str, location: Location) -> Transaction:
        text = line.lstrip(" \t")
        if text.startswith(";"):
            entry.items.append(Comment(text[1:].strip()))
        else:
            entry.items.append(self.posting(strip_comment(text), location))
        return entry

    def posting(self, text: str, location: Location) -> Posting:
        separator = POSTING_SEPARATOR_RE.search(text)
        account = text[: separator.start()] if separator else text
        if not valid_account(account):
            raise EntryError(f"invalid account name: {account}")
        if not separator:
            return Posting(account, None, None, location)
        amount, cost = self.amount_field(text[separator.end():].strip())
        return Posting(account, amount, cost, location)

    def amount_field(self, text: str) -> tuple[Amount, Cost | None]:
        tokens = text.split()
        amount = self.amount(tokens[:2]) if len(tokens) in (2, 5) else None
        cost = None
        if amount is not None and len(tokens) == 5:
            price = self.amount(tokens[3:]) if tokens[2] in ("@", "@@") else None
            if price is None or price.quantity < 0 or price.commodity == amount.commodity:
                amount = None
            else:
                cost = Cost(tokens[2] == "@@", price)
        if amount is None:
            raise EntryError(f"invalid amount: {text}")
        return amount, cost

    def amount(self, tokens: list[str]) -> Amount | None:
        number = parse_number(tokens[0])
        if number is None or not is_commodity(tokens[1]):
            return None
        self.note_precision(tokens[1], number[1])
        return Amount(number[0], tokens[1])

    def note_precision(self, commodity: str, decimals: int) -> None:
        precision = self.journal.precision
        precision[commodity] = max(precision.get(commodity, 0), decimals)

    def finish(self, entry: Transaction | None) -> None:
        if entry is None:
            return
        try:
            balance_transaction(entry)
        except EntryError as exc:
            self.error(entry.location, exc.message)
            return
        self.journal.transactions.append(entry)

    # -- directives ----------------------------------------------------------------------

    def directive(self, line: str, location: Location, display: str, chain: list[str]) -> None:
        text = strip_comment(line)
        name, rest = (text.split(None, 1) + [""])[:2]
        rest = rest.strip()
        if name == "include":
            self.include(rest, display, chain)
        elif name == "account":
            if not rest:
                raise EntryError("invalid account directive")
            if not valid_account(rest):
                raise EntryError(f"invalid account name: {rest}")
            self.journal.declared.add(rest)
        elif name == "P":
            self.price(rest.split(), location)
        else:
            raise EntryError(f"unknown directive: {name}")

    def include(self, path: str, display: str, chain: list[str]) -> None:
        if not path:
            raise EntryError("invalid include directive")
        child_display = os.path.join(os.path.dirname(display), path)
        real = os.path.realpath(child_display)
        if real in chain:
            raise EntryError(f"include cycle: {path}")
        try:
            text = read_text(child_display)
        except OSError:
            raise EntryError(f"cannot read include: {path}") from None
        self.read_lines(text, child_display, chain + [real])

    def price(self, fields: list[str], location: Location) -> None:
        if len(fields) != 4:
            raise EntryError("invalid price directive")
        date_text, commodity, number_text, price_commodity = fields
        date = parse_date(date_text)
        if date is None:
            raise EntryError(f"invalid date: {date_text}")
        if not is_commodity(commodity):
            raise EntryError(f"invalid commodity: {commodity}")
        number = parse_number(number_text)
        if number is None or number[0] < 0 or not is_commodity(price_commodity):
            raise EntryError(f"invalid amount: {number_text} {price_commodity}")
        if price_commodity == commodity:
            raise EntryError("invalid price directive")
        self.note_precision(commodity, 0)
        self.note_precision(price_commodity, number[1])
        self.journal.prices.append(Price(date, commodity, Amount(number[0], price_commodity), location.order))

    # -- strict mode -------------------------------------------------------------------

    def check_declared(self) -> None:
        for transaction in self.journal.transactions:
            for posting in transaction.postings:
                if posting.account not in self.journal.declared:
                    self.error(posting.location, f"undeclared account: {posting.account}")


def read_text(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def balance_transaction(transaction: Transaction) -> None:
    """Validate the postings and infer a missing amount (SPEC 1.1)."""
    postings = transaction.postings
    if len(postings) < 2:
        raise EntryError("transaction has fewer than two postings")
    elided = [p for p in postings if p.amount is None]
    if len(elided) > 1:
        raise EntryError("multiple postings without amount")
    residual = Balance()
    for posting in postings:
        if posting.amount is not None:
            residual.add(posting.weight())
    open_amounts = residual.nonzero()
    if elided:
        if not open_amounts:
            raise EntryError("cannot infer amount: transaction already balances")
        if len(open_amounts) > 1:
            raise EntryError("cannot infer amount: residual in multiple commodities")
        elided[0].amount = Amount(-open_amounts[0].quantity, open_amounts[0].commodity)
        elided[0].elided = True
    elif open_amounts:
        listed = ", ".join(f"{a.quantity:f} {a.commodity}" for a in open_amounts)
        raise EntryError(f"transaction does not balance: residual {listed}")


def load(path: str, strict: bool = False) -> Journal:
    return Parser(strict).parse(path)

