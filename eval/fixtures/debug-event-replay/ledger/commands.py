"""Command handlers.

Each handler checks a command against the current `WalletBook` and returns the event to
append. Handlers never mutate state; the service appends the event and then applies it.
"""

from __future__ import annotations

import re
from decimal import Decimal

from .errors import InsufficientFunds, WalletExists
from .events import DEPOSITED, WALLET_OPENED, WITHDRAWN, PendingEvent
from .money import format_amount, parse_amount
from .wallet import WalletBook

WALLET_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")


def validate_wallet_id(wallet_id: object) -> str:
    if not isinstance(wallet_id, str) or not WALLET_ID.fullmatch(wallet_id):
        raise ValueError(f"invalid wallet id: {wallet_id!r}")
    return wallet_id


def open_wallet(book: WalletBook, wallet_id: str) -> PendingEvent:
    validate_wallet_id(wallet_id)
    if wallet_id in book:
        raise WalletExists(wallet_id)
    return PendingEvent(WALLET_OPENED, wallet_id)


def deposit(book: WalletBook, wallet_id: str, amount: str | Decimal) -> PendingEvent:
    book.get(wallet_id)
    value = parse_amount(amount)
    return PendingEvent(DEPOSITED, wallet_id, {"amount": format_amount(value)})


def withdraw(book: WalletBook, wallet_id: str, amount: str | Decimal) -> PendingEvent:
    wallet = book.get(wallet_id)
    value = parse_amount(amount)
    if wallet.balance < value:
        raise InsufficientFunds(
            f"wallet {wallet_id} has {format_amount(wallet.balance)}, needs {format_amount(value)}"
        )
    return PendingEvent(WITHDRAWN, wallet_id, {"amount": format_amount(value)})
