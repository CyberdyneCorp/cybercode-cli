"""Payment-provider webhook ingestion.

Providers deliver at-least-once and retry on timeouts, so every delivery carries an
idempotency key and a key is applied at most once.
"""

from __future__ import annotations

from typing import Any

from .events import CHARGEBACK_RECEIVED, TOPUP_RECEIVED, Event, PendingEvent
from .money import format_amount, parse_amount
from .wallet import WalletBook

EVENT_TYPE_BY_WEBHOOK = {
    "topup.succeeded": TOPUP_RECEIVED,
    "chargeback.created": CHARGEBACK_RECEIVED,
}


class WebhookIngestor:
    def __init__(self) -> None:
        # Keys of every applied delivery. The key is stored in the event it produced, so
        # the service restores this set from the full log on startup.
        self._seen: set[str] = set()

    def is_duplicate(self, key: str) -> bool:
        return key in self._seen

    def prepare(self, book: WalletBook, payload: dict[str, Any]) -> PendingEvent | None:
        """Return the event a delivery produces, or None if it was already applied.

        Raises ValueError (or a LedgerError) for a delivery that cannot be applied.
        """
        if not isinstance(payload, dict):
            raise ValueError("webhook payload must be an object")
        key = payload.get("idempotency_key")
        if not isinstance(key, str) or not key:
            raise ValueError("webhook payload needs a non-empty idempotency_key")
        if self.is_duplicate(key):
            return None
        event_type = EVENT_TYPE_BY_WEBHOOK.get(payload.get("type"))
        if event_type is None:
            raise ValueError(f"unsupported webhook type: {payload.get('type')!r}")
        wallet_id = payload.get("wallet_id")
        book.get(wallet_id)
        amount = parse_amount(payload.get("amount"))
        return PendingEvent(
            event_type, wallet_id, {"amount": format_amount(amount), "idempotency_key": key}
        )

    def record(self, event: Event) -> None:
        """Remember the idempotency key of an applied event (if it has one)."""
        if event.idempotency_key is not None:
            self._seen.add(event.idempotency_key)

    def record_all(self, events: list[Event]) -> None:
        for event in events:
            self.record(event)
