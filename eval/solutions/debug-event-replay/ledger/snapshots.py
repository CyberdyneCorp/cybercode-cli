"""Snapshots of the wallet state.

`snapshot.json` holds the state of every wallet after some event, so that startup only has
to replay the events after it:

    {"format": 1, "next_seq": 42, "wallets": {"alice": {"balance": ..., "version": 40}}}

`next_seq` is the sequence number of the first event the snapshot does NOT include.
The file is replaced atomically, so a crash while saving leaves the previous snapshot.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path

from .errors import CorruptLog
from .money import format_amount
from .wallet import Wallet, WalletBook

SNAPSHOT_FILENAME = "snapshot.json"
FORMAT_VERSION = 1


@dataclass
class Snapshot:
    book: WalletBook
    next_seq: int


def save(directory: str | os.PathLike, book: WalletBook) -> Snapshot:
    """Write a snapshot of `book`, which has applied every event up to `book.seq`."""
    path = Path(directory) / SNAPSHOT_FILENAME
    payload = {
        "format": FORMAT_VERSION,
        "next_seq": book.seq + 1,
        "wallets": {
            wallet_id: {"balance": format_amount(wallet.balance), "version": wallet.version}
            for wallet_id, wallet in book.wallets.items()
        },
    }
    tmp = path.with_suffix(".tmp")
    # Balances are decimal strings: a JSON number would go through float and lose exactness.
    tmp.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")
    os.replace(tmp, path)
    return Snapshot(book, payload["next_seq"])


def _decimal(value: object) -> Decimal:
    if not isinstance(value, str):
        raise TypeError(f"snapshot balance must be a decimal string, got {value!r}")
    return Decimal(value)


def load(directory: str | os.PathLike) -> Snapshot | None:
    """Return the latest snapshot, or None when there is none."""
    path = Path(directory) / SNAPSHOT_FILENAME
    if not path.exists():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
        if payload.get("format") != FORMAT_VERSION:
            raise CorruptLog(f"unsupported snapshot format {payload.get('format')!r}")
        wallets = {
            wallet_id: Wallet(wallet_id, _decimal(raw["balance"]), int(raw["version"]))
            for wallet_id, raw in payload["wallets"].items()
        }
        next_seq = int(payload["next_seq"])
    except (ValueError, KeyError, TypeError, AttributeError, ArithmeticError) as exc:
        raise CorruptLog(f"cannot read snapshot {path}") from exc
    return Snapshot(WalletBook(wallets, seq=next_seq - 1), next_seq)
