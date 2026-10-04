"""WalletService: the facade the API layer and the operator tools use."""

from __future__ import annotations

import os
from decimal import Decimal
from pathlib import Path
from typing import Any

from . import commands, snapshots
from .clock import Clock, system_clock
from .events import Event, PendingEvent
from .projections import Statement, StatementProjection
from .store import EventStore
from .wallet import WalletBook
from .webhooks import WebhookIngestor


class WalletService:
    def __init__(
        self,
        data_dir: str | os.PathLike,
        clock: Clock = system_clock,
        use_snapshot: bool = True,
    ):
        self._dir = Path(data_dir)
        self._store = EventStore(self._dir, clock)
        self._book = self._load_book(use_snapshot)
        self._statements = StatementProjection()
        self._webhooks = WebhookIngestor()
        self.rebuild_projections()

    def _load_book(self, use_snapshot: bool) -> WalletBook:
        snapshot = snapshots.load(self._dir) if use_snapshot else None
        if snapshot is None:
            book, start = WalletBook(), 0
        else:
            book, start = snapshot.book, snapshot.next_seq
        book.apply_all(self._store.read(after_seq=start))
        return book

    # -- commands ---------------------------------------------------------------------

    def open_wallet(self, wallet_id: str) -> int:
        return self._append(commands.open_wallet(self._book, wallet_id)).seq

    def deposit(self, wallet_id: str, amount: str | Decimal) -> int:
        return self._append(commands.deposit(self._book, wallet_id, amount)).seq

    def withdraw(self, wallet_id: str, amount: str | Decimal) -> int:
        return self._append(commands.withdraw(self._book, wallet_id, amount)).seq

    def handle_webhook(self, payload: dict[str, Any]) -> str:
        pending = self._webhooks.prepare(self._book, payload)
        if pending is None:
            return "duplicate"
        event = self._append(pending)
        self._webhooks.record(event)
        return "applied"

    def _append(self, pending: PendingEvent) -> Event:
        event = self._store.append(pending)
        self._book.apply(event)
        self._statements.apply(event)
        return event

    # -- queries ----------------------------------------------------------------------

    def balance(self, wallet_id: str) -> Decimal:
        return self._book.get(wallet_id).balance

    def wallets(self) -> list[str]:
        return sorted(self._book.wallets)

    @property
    def last_seq(self) -> int:
        return self._store.last_seq

    def statement(self, wallet_id: str, month: str) -> Statement:
        self._book.get(wallet_id)
        return self._statements.statement(wallet_id, month)

    # -- maintenance ------------------------------------------------------------------

    def take_snapshot(self) -> int:
        snapshots.save(self._dir, self._book)
        return self._book.seq

    def rebuild_projections(self) -> None:
        self._statements.rebuild(self._store.read())
