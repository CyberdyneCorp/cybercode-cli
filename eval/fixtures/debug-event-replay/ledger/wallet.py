"""The wallet aggregate.

`WalletBook` holds every wallet and is rebuilt by applying events one by one. Applying an
event never validates business rules (events are facts that already happened); validation
belongs to the command handlers.
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from .errors import CorruptLog, UnknownWallet
from .events import WALLET_OPENED, Event


@dataclass
class Wallet:
    wallet_id: str
    balance: Decimal = Decimal("0")
    version: int = 0  # sequence number of the last event applied to this wallet

    def apply(self, event: Event) -> None:
        self.balance += event.signed_amount
        self.version = event.seq


class WalletBook:
    def __init__(self, wallets: dict[str, Wallet] | None = None, seq: int = 0):
        self.wallets: dict[str, Wallet] = dict(wallets or {})
        self.seq = seq  # sequence number of the last event applied to the book

    def get(self, wallet_id: str) -> Wallet:
        try:
            return self.wallets[wallet_id]
        except KeyError:
            raise UnknownWallet(wallet_id) from None

    def __contains__(self, wallet_id: str) -> bool:
        return wallet_id in self.wallets

    def apply(self, event: Event) -> None:
        if event.type == WALLET_OPENED:
            if event.wallet_id in self.wallets:
                raise CorruptLog(f"wallet {event.wallet_id} opened twice (seq {event.seq})")
            self.wallets[event.wallet_id] = Wallet(event.wallet_id, version=event.seq)
        else:
            self.get(event.wallet_id).apply(event)
        self.seq = event.seq

    def apply_all(self, events: list[Event]) -> None:
        for event in events:
            self.apply(event)

    def balances(self) -> dict[str, Decimal]:
        return {wallet_id: self.wallets[wallet_id].balance for wallet_id in sorted(self.wallets)}
