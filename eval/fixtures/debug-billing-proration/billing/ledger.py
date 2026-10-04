"""Per-customer ledger of issued invoices (used by the account page and statements).

The ledger never recomputes amounts: it only adds up invoice totals that were already
rounded by invoice.py. Each customer's balance is kept per currency.
"""
from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass
from decimal import Decimal

from .invoice import Invoice
from .money import ZERO, format_amount


@dataclass(frozen=True)
class Entry:
    number: str
    customer: str
    currency: str
    total: Decimal
    first_day: str


class Ledger:
    def __init__(self, prefix: str = "INV"):
        self.prefix = prefix
        self._entries: list[Entry] = []

    def record(self, invoice: Invoice) -> Entry:
        """Assign the next invoice number and remember the invoice total."""
        number = f"{self.prefix}-{len(self._entries) + 1:06d}"
        first_day = min(line.start for line in invoice.lines).isoformat() if invoice.lines else ""
        entry = Entry(number, invoice.customer, invoice.currency, invoice.total, first_day)
        self._entries.append(entry)
        return entry

    def entries(self, customer: str | None = None) -> list[Entry]:
        return [e for e in self._entries if customer is None or e.customer == customer]

    def balances(self, customer: str) -> dict[str, Decimal]:
        """Amount owed by ``customer`` per currency (negative means we owe them)."""
        totals: dict[str, Decimal] = defaultdict(lambda: ZERO)
        for entry in self.entries(customer):
            totals[entry.currency] += entry.total
        return dict(sorted(totals.items()))

    def statement(self, customer: str) -> str:
        """Plain-text statement: one row per invoice, then one balance row per currency."""
        rows = [f"STATEMENT {customer}"]
        for entry in self.entries(customer):
            rows.append(f"{entry.number}  {entry.first_day:<10}  "
                        f"{format_amount(entry.total, entry.currency):>12} {entry.currency}")
        for currency, balance in self.balances(customer).items():
            rows.append(f"{'Balance':<30}{format_amount(balance, currency):>12} {currency}")
        return "\n".join(rows) + "\n"
