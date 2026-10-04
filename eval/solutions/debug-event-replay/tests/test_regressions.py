import json
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from pathlib import Path

from ledger import WalletService

TOKYO = timezone(timedelta(hours=9))
NEW_YORK = timezone(timedelta(hours=-5))


class Clock:
    def __init__(self, now: datetime):
        self.now = now

    def __call__(self) -> datetime:
        return self.now


class RegressionTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.data_dir = self._tmp.name
        self.clock = Clock(datetime(2026, 3, 2, 9, 0, tzinfo=timezone.utc))

    def service(self, **kwargs) -> WalletService:
        return WalletService(self.data_dir, clock=self.clock, **kwargs)

    def test_large_amounts_survive_snapshot(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "12345678901234567.89")
        service.take_snapshot()
        self.assertEqual(self.service().balance("a"), Decimal("12345678901234567.89"))
        raw = json.loads((Path(self.data_dir) / "snapshot.json").read_text())
        self.assertEqual(raw["wallets"]["a"]["balance"], "12345678901234567.89")

    def test_first_event_after_snapshot_survives_restart(self):
        service = self.service()
        service.open_wallet("a")
        service.take_snapshot()
        service.deposit("a", "5")
        self.assertEqual(self.service().balance("a"), Decimal("5"))

    def test_webhook_key_survives_restart_and_snapshot(self):
        service = self.service()
        service.open_wallet("a")
        payload = {"idempotency_key": "k1", "type": "topup.succeeded", "wallet_id": "a", "amount": "3"}
        self.assertEqual(service.handle_webhook(payload), "applied")
        service.take_snapshot()
        self.assertEqual(self.service().handle_webhook(payload), "duplicate")
        code = (
            "import sys; from ledger import WalletService;"
            f"print(WalletService(sys.argv[1]).handle_webhook({payload!r}))"
        )
        out = subprocess.run([sys.executable, "-c", code, self.data_dir], capture_output=True, text=True, check=True)
        self.assertEqual(out.stdout.strip(), "duplicate")
        self.assertEqual(self.service().balance("a"), Decimal("3"))

    def test_replay_uses_sequence_order_not_timestamps(self):
        self.clock.now = datetime(2026, 3, 2, 9, 0, tzinfo=TOKYO)  # 00:00 UTC
        service = self.service()
        service.open_wallet("a")
        self.clock.now = datetime(2026, 3, 1, 19, 30, tzinfo=NEW_YORK)  # 00:30 UTC, later
        service.deposit("a", "1")
        self.assertEqual(self.service(use_snapshot=False).balance("a"), Decimal("1"))

    def test_statement_month_is_utc(self):
        self.clock.now = datetime(2026, 4, 1, 5, 0, tzinfo=TOKYO)  # 2026-03-31 20:00 UTC
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "2")
        self.assertEqual(service.statement("a", "2026-03").credits, Decimal("2"))
        self.assertEqual(service.statement("a", "2026-04").lines, ())

    def test_rebuild_projections_is_idempotent(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "2")
        before = service.statement("a", "2026-03")
        service.rebuild_projections()
        service.rebuild_projections()
        self.assertEqual(service.statement("a", "2026-03"), before)


if __name__ == "__main__":
    unittest.main()
