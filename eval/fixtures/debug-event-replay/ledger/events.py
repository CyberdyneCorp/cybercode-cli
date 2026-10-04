"""Event records and their JSON Lines encoding.

Each line of `events.jsonl` is one JSON object:

    {"data": {"amount": "12.30"}, "recorded_at": "2026-03-01T10:00:00+00:00",
     "seq": 7, "type": "Deposited", "wallet_id": "alice"}

Amounts are stored as decimal strings so they survive the round trip exactly.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from decimal import Decimal
from typing import Any

from .errors import CorruptLog

WALLET_OPENED = "WalletOpened"
DEPOSITED = "Deposited"
WITHDRAWN = "Withdrawn"
TOPUP_RECEIVED = "TopUpReceived"
CHARGEBACK_RECEIVED = "ChargebackReceived"

# Statement kind of every event type that moves money, by direction.
CREDIT_KINDS = {DEPOSITED: "deposit", TOPUP_RECEIVED: "topup"}
DEBIT_KINDS = {WITHDRAWN: "withdrawal", CHARGEBACK_RECEIVED: "chargeback"}

EVENT_TYPES = frozenset({WALLET_OPENED, *CREDIT_KINDS, *DEBIT_KINDS})


@dataclass(frozen=True)
class Event:
    seq: int
    type: str
    wallet_id: str
    recorded_at: str
    data: dict[str, Any] = field(default_factory=dict)

    @property
    def amount(self) -> Decimal:
        return Decimal(self.data["amount"])

    @property
    def signed_amount(self) -> Decimal:
        """Positive for credits, negative for debits, zero for other events."""
        if self.type in CREDIT_KINDS:
            return self.amount
        if self.type in DEBIT_KINDS:
            return -self.amount
        return Decimal("0")

    @property
    def idempotency_key(self) -> str | None:
        return self.data.get("idempotency_key")


@dataclass(frozen=True)
class PendingEvent:
    """An event produced by a command handler, not yet assigned a sequence number."""

    type: str
    wallet_id: str
    data: dict[str, Any] = field(default_factory=dict)


def encode(event: Event) -> str:
    record = {
        "seq": event.seq,
        "type": event.type,
        "wallet_id": event.wallet_id,
        "recorded_at": event.recorded_at,
        "data": event.data,
    }
    return json.dumps(record, sort_keys=True, separators=(",", ":"))


def decode(line: str) -> Event:
    try:
        record = json.loads(line)
        event = Event(
            seq=int(record["seq"]),
            type=record["type"],
            wallet_id=record["wallet_id"],
            recorded_at=record["recorded_at"],
            data=dict(record.get("data") or {}),
        )
    except (ValueError, KeyError, TypeError) as exc:
        raise CorruptLog(f"cannot decode event: {line[:80]!r}") from exc
    if event.type not in EVENT_TYPES:
        raise CorruptLog(f"unknown event type {event.type!r} at seq {event.seq}")
    return event
