"""Monthly statement projection.

The projection is fed every event in sequence order (live as events are appended, or in
bulk by `rebuild`) and answers `statement(wallet_id, month)` queries.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from decimal import Decimal

from .clock import to_utc
from .errors import UnknownWallet
from .events import CREDIT_KINDS, DEBIT_KINDS, WALLET_OPENED, Event

MONTH = re.compile(r"\d{4}-(0[1-9]|1[0-2])")
ZERO = Decimal("0")


@dataclass(frozen=True)
class StatementLine:
    seq: int
    kind: str
    amount: Decimal
    balance: Decimal


@dataclass(frozen=True)
class Statement:
    wallet_id: str
    month: str
    opening: Decimal
    credits: Decimal
    debits: Decimal
    closing: Decimal
    lines: tuple[StatementLine, ...]


@dataclass(frozen=True)
class _Entry:
    seq: int
    month: str
    kind: str
    signed_amount: Decimal


def statement_month(recorded_at: str) -> str:
    """The statement month ("YYYY-MM") an event recorded at `recorded_at` belongs to."""
    return to_utc(recorded_at).strftime("%Y-%m")


class StatementProjection:
    def __init__(self) -> None:
        self._entries: dict[str, list[_Entry]] = {}

    def apply(self, event: Event) -> None:
        if event.type == WALLET_OPENED:
            self._entries.setdefault(event.wallet_id, [])
            return
        kind = CREDIT_KINDS.get(event.type) or DEBIT_KINDS.get(event.type)
        if kind is None:
            return
        entry = _Entry(event.seq, statement_month(event.recorded_at), kind, event.signed_amount)
        self._entries.setdefault(event.wallet_id, []).append(entry)

    def rebuild(self, events: list[Event]) -> None:
        """Discard all state and rebuild the projection from the full log."""
        self._entries = {}
        for event in events:
            self.apply(event)

    def statement(self, wallet_id: str, month: str) -> Statement:
        if not isinstance(month, str) or not MONTH.fullmatch(month):
            raise ValueError(f"invalid month: {month!r}")
        if wallet_id not in self._entries:
            raise UnknownWallet(wallet_id)
        entries = self._entries[wallet_id]
        opening = sum((e.signed_amount for e in entries if e.month < month), ZERO)
        balance = opening
        credits = debits = ZERO
        lines = []
        for entry in entries:
            if entry.month != month:
                continue
            balance += entry.signed_amount
            if entry.signed_amount > 0:
                credits += entry.signed_amount
            else:
                debits -= entry.signed_amount
            lines.append(StatementLine(entry.seq, entry.kind, abs(entry.signed_amount), balance))
        return Statement(wallet_id, month, opening, credits, debits, balance, tuple(lines))
