import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from decimal import Decimal

from ledger import InsufficientFunds, WalletService


class ManualClock:
    def __init__(self, start: datetime):
        self.now = start

    def advance(self, **delta) -> None:
        self.now += timedelta(**delta)

    def __call__(self) -> datetime:
        return self.now


class WalletServiceTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.data_dir = self._tmp.name
        self.clock = ManualClock(datetime(2026, 3, 2, 9, 0, tzinfo=timezone.utc))

    def service(self, **kwargs) -> WalletService:
        return WalletService(self.data_dir, clock=self.clock, **kwargs)

    def test_deposit_and_withdraw(self):
        service = self.service()
        self.assertEqual(service.open_wallet("alice"), 1)
        service.deposit("alice", "25.00")
        self.clock.advance(minutes=5)
        self.assertEqual(service.withdraw("alice", "7.55"), 3)
        self.assertEqual(service.balance("alice"), Decimal("17.45"))
        self.assertEqual(service.last_seq, 3)

    def test_insufficient_funds(self):
        service = self.service()
        service.open_wallet("bob")
        service.deposit("bob", "5")
        with self.assertRaises(InsufficientFunds):
            service.withdraw("bob", "5.01")
        self.assertEqual(service.balance("bob"), Decimal("5"))
        self.assertEqual(service.last_seq, 2)

    def test_duplicate_webhook_is_ignored(self):
        service = self.service()
        service.open_wallet("carol")
        payload = {"idempotency_key": "evt_1", "type": "topup.succeeded", "wallet_id": "carol", "amount": "10.00"}
        self.assertEqual(service.handle_webhook(payload), "applied")
        self.assertEqual(service.handle_webhook(payload), "duplicate")
        self.assertEqual(service.balance("carol"), Decimal("10.00"))

    def test_balance_survives_restart(self):
        service = self.service()
        service.open_wallet("dave")
        for _ in range(3):
            service.deposit("dave", "0.10")
        service.take_snapshot()
        restarted = self.service()
        self.assertEqual(restarted.balance("dave"), Decimal("0.30"))

    def test_monthly_statement(self):
        service = self.service()
        service.open_wallet("erin")
        service.deposit("erin", "100")
        self.clock.advance(days=1)
        service.withdraw("erin", "40.50")
        statement = service.statement("erin", "2026-03")
        self.assertEqual(statement.opening, Decimal("0"))
        self.assertEqual(statement.credits, Decimal("100"))
        self.assertEqual(statement.debits, Decimal("40.50"))
        self.assertEqual(statement.closing, Decimal("59.50"))
        self.assertEqual([(line.seq, line.kind) for line in statement.lines], [(2, "deposit"), (3, "withdrawal")])


if __name__ == "__main__":
    unittest.main()
