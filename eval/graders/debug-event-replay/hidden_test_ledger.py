"""Hidden tests for debug-event-replay: targeted regressions plus randomized command
sequences with snapshots, restarts and projection rebuilds, checked against a model of the
README contract."""

import random
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from decimal import Decimal

from ledger import InsufficientFunds, InvalidAmount, UnknownWallet, WalletService

UTC = timezone.utc
TOKYO = timezone(timedelta(hours=9))
NEW_YORK = timezone(timedelta(hours=-5))
OFFSETS = [UTC, TOKYO, NEW_YORK, timezone(timedelta(hours=5, minutes=30)),
           timezone(timedelta(hours=-8)), timezone(timedelta(hours=1)),
           timezone(timedelta(hours=-3, minutes=-30)), timezone(timedelta(hours=13))]
CREDIT = {"deposit", "topup"}


class Clock:
    """Returns `instant` rendered in `offset`; the test moves both explicitly."""

    def __init__(self, instant: datetime, offset=UTC):
        self.instant = instant
        self.offset = offset

    def __call__(self) -> datetime:
        return self.instant.astimezone(self.offset)

    def month(self) -> str:
        return self.instant.astimezone(UTC).strftime("%Y-%m")


class Model:
    """The README contract, computed independently of the ledger package."""

    def __init__(self):
        self.entries: dict[str, list[tuple[int, str, str, Decimal]]] = {}
        self.last_seq = 0
        self.keys: set[str] = set()

    def append(self) -> int:
        self.last_seq += 1
        return self.last_seq

    def open(self, wallet_id):
        self.entries[wallet_id] = []
        return self.append()

    def move(self, wallet_id, kind, amount, month):
        seq = self.append()
        self.entries[wallet_id].append((seq, month, kind, Decimal(amount)))
        return seq

    def balance(self, wallet_id) -> Decimal:
        return sum(((a if k in CREDIT else -a) for _, _, k, a in self.entries[wallet_id]), Decimal(0))

    def statement(self, wallet_id, month):
        entries = self.entries[wallet_id]
        opening = sum(((a if k in CREDIT else -a) for _, m, k, a in entries if m < month), Decimal(0))
        running, credits, debits, lines = opening, Decimal(0), Decimal(0), []
        for seq, m, kind, amount in sorted(entries):
            if m != month:
                continue
            if kind in CREDIT:
                running += amount
                credits += amount
            else:
                running -= amount
                debits += amount
            lines.append((seq, kind, amount, running))
        return opening, credits, debits, running, lines

    def months(self):
        months = sorted({m for entries in self.entries.values() for _, m, _, _ in entries})
        return months + ["1999-12", "2099-01"]


def webhook(key, kind, wallet_id, amount):
    return {"idempotency_key": key, "type": kind, "wallet_id": wallet_id, "amount": amount}


class LedgerCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.data_dir = self._tmp.name
        self.clock = Clock(datetime(2026, 3, 2, 9, 0, tzinfo=UTC))

    def service(self, use_snapshot=True) -> WalletService:
        return WalletService(self.data_dir, clock=self.clock, use_snapshot=use_snapshot)

    def at(self, *args, tz=UTC):
        self.clock.instant = datetime(*args, tzinfo=tz)
        self.clock.offset = tz

    def assert_statement(self, statement, opening, credits, debits, closing, lines, msg=None):
        got = (statement.opening, statement.credits, statement.debits, statement.closing,
               [(l.seq, l.kind, l.amount, l.balance) for l in statement.lines])
        want = (Decimal(opening), Decimal(credits), Decimal(debits), Decimal(closing),
                [(s, k, Decimal(a), Decimal(b)) for s, k, a, b in lines])
        self.assertEqual(got, want, msg)
        self.assertIsInstance(statement.lines, tuple, msg)

    def run_python(self, code, *args):
        result = subprocess.run([sys.executable, "-c", code, self.data_dir, *args],
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout


class SnapshotTest(LedgerCase):
    def test_small_amounts_round_trip(self):
        service = self.service()
        service.open_wallet("a")
        for amount in ["0.10", "0.20", "0.07", "1.01"]:
            service.deposit("a", amount)
        service.take_snapshot()
        self.assertEqual(self.service().balance("a"), Decimal("1.38"))

    def test_large_amounts_round_trip(self):
        service = self.service()
        service.open_wallet("a")
        service.open_wallet("b")
        service.deposit("a", "12345678901234567.89")
        service.deposit("b", "98765432109876.54")
        service.withdraw("b", "0.01")
        service.take_snapshot()
        restarted = self.service()
        self.assertEqual(restarted.balance("a"), Decimal("12345678901234567.89"))
        self.assertEqual(restarted.balance("b"), Decimal("98765432109876.53"))

    def test_take_snapshot_returns_last_seq(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "1")
        self.assertEqual(service.take_snapshot(), 2)
        service.deposit("a", "1")
        self.assertEqual(service.take_snapshot(), 3)

    def test_first_event_after_snapshot_survives_restart(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "10")
        service.take_snapshot()
        service.deposit("a", "5")
        restarted = self.service()
        self.assertEqual(restarted.balance("a"), Decimal("15"))
        self.assertEqual(restarted.last_seq, 3)

    def test_wallet_opened_right_after_snapshot(self):
        service = self.service()
        service.open_wallet("a")
        service.take_snapshot()
        service.open_wallet("b")
        service.deposit("b", "2")
        self.assertEqual(self.service().wallets(), ["a", "b"])
        self.assertEqual(self.service().balance("b"), Decimal("2"))

    def test_snapshot_of_empty_log(self):
        service = self.service()
        self.assertEqual(service.take_snapshot(), 0)
        service.open_wallet("a")
        service.deposit("a", "4")
        self.assertEqual(self.service().balance("a"), Decimal("4"))

    def test_repeated_snapshots_and_restarts(self):
        service = self.service()
        service.open_wallet("a")
        total = Decimal(0)
        for round_ in range(5):
            for i in range(3):
                amount = f"{round_}.{i}3"
                service.deposit("a", amount)
                total += Decimal(amount)
            service.take_snapshot()
            service.deposit("a", "0.01")
            total += Decimal("0.01")
            service = self.service()
            self.assertEqual(service.balance("a"), total, f"round {round_}")
        self.assertEqual(self.service(use_snapshot=False).balance("a"), total)

    def test_snapshot_does_not_double_count(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "3")
        service.take_snapshot()
        for _ in range(3):
            self.assertEqual(self.service().balance("a"), Decimal("3"))


class OrderingTest(LedgerCase):
    def test_replay_in_sequence_order_with_mixed_offsets(self):
        self.at(2026, 3, 2, 9, 0, tz=TOKYO)            # 2026-03-02 00:00 UTC
        service = self.service()
        service.open_wallet("a")
        self.at(2026, 3, 1, 19, 30, tz=NEW_YORK)       # 2026-03-02 00:30 UTC (later)
        service.deposit("a", "7")
        for use_snapshot in (False, True):
            self.assertEqual(self.service(use_snapshot).balance("a"), Decimal("7"))

    def test_tail_after_snapshot_in_sequence_order(self):
        self.at(2026, 5, 10, 12, 0)
        service = self.service()
        service.open_wallet("a")
        service.take_snapshot()
        self.at(2026, 5, 10, 22, 0, tz=TOKYO)          # 13:00 UTC
        service.open_wallet("b")
        self.at(2026, 5, 10, 9, 0, tz=NEW_YORK)        # 14:00 UTC
        service.deposit("b", "1.50")
        self.assertEqual(self.service().balance("b"), Decimal("1.50"))

    def test_statement_lines_in_sequence_order_after_restart(self):
        self.at(2026, 3, 10, 23, 0, tz=TOKYO)          # 14:00 UTC
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "100")                     # seq 2
        self.at(2026, 3, 10, 10, 0, tz=NEW_YORK)       # 15:00 UTC
        service.withdraw("a", "60")                     # seq 3
        self.at(2026, 3, 10, 16, 0)                     # 16:00 UTC
        service.deposit("a", "1")                       # seq 4
        want = [(2, "deposit", "100", "100"), (3, "withdrawal", "60", "40"), (4, "deposit", "1", "41")]
        self.assert_statement(service.statement("a", "2026-03"), 0, 101, 60, 41, want, "live")
        for use_snapshot in (False, True):
            restarted = self.service(use_snapshot)
            self.assert_statement(restarted.statement("a", "2026-03"), 0, 101, 60, 41, want, "restart")
            restarted.rebuild_projections()
            self.assert_statement(restarted.statement("a", "2026-03"), 0, 101, 60, 41, want, "rebuild")


class StatementTest(LedgerCase):
    def test_month_is_utc_month(self):
        self.at(2026, 4, 1, 5, 0, tz=TOKYO)            # 2026-03-31 20:00 UTC
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "2")
        self.at(2026, 3, 31, 21, 0, tz=NEW_YORK)       # 2026-04-01 02:00 UTC
        service.deposit("a", "3")
        self.assert_statement(service.statement("a", "2026-03"), 0, 2, 0, 2, [(2, "deposit", "2", "2")])
        self.assert_statement(service.statement("a", "2026-04"), 2, 3, 0, 5, [(3, "deposit", "3", "5")])
        restarted = self.service(use_snapshot=False)
        self.assert_statement(restarted.statement("a", "2026-03"), 0, 2, 0, 2, [(2, "deposit", "2", "2")])

    def test_year_boundary_in_utc(self):
        self.at(2026, 12, 31, 20, 0, tz=NEW_YORK)      # 2027-01-01 01:00 UTC
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "9")
        self.assert_statement(service.statement("a", "2026-12"), 0, 0, 0, 0, [])
        self.assert_statement(service.statement("a", "2027-01"), 0, 9, 0, 9, [(2, "deposit", "9", "9")])

    def test_rebuild_projections_twice(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "10")
        service.withdraw("a", "4")
        want = [(2, "deposit", "10", "10"), (3, "withdrawal", "4", "6")]
        for _ in range(3):
            service.rebuild_projections()
            self.assert_statement(service.statement("a", "2026-03"), 0, 10, 4, 6, want)
        self.assertEqual(service.balance("a"), Decimal("6"))

    def test_rebuild_then_live_events(self):
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "10")
        service.rebuild_projections()
        service.handle_webhook(webhook("k1", "chargeback.created", "a", "15"))
        want = [(2, "deposit", "10", "10"), (3, "chargeback", "15", "-5")]
        self.assert_statement(service.statement("a", "2026-03"), 0, 10, 15, -5, want)
        self.assertEqual(service.balance("a"), Decimal("-5"))

    def test_opening_and_empty_months(self):
        self.at(2026, 1, 15, 12, 0)
        service = self.service()
        service.open_wallet("a")
        service.deposit("a", "50")
        self.at(2026, 3, 15, 12, 0)
        service.withdraw("a", "20.25")
        self.assert_statement(service.statement("a", "2026-02"), 50, 0, 0, 50, [])
        self.assert_statement(service.statement("a", "2026-03"), 50, 0, "20.25", "29.75",
                              [(3, "withdrawal", "20.25", "29.75")])
        self.assert_statement(service.statement("a", "2025-12"), 0, 0, 0, 0, [])

    def test_statement_errors(self):
        service = self.service()
        service.open_wallet("a")
        with self.assertRaises(UnknownWallet):
            service.statement("nobody", "2026-03")
        with self.assertRaises(ValueError):
            service.statement("a", "2026-13")


class WebhookTest(LedgerCase):
    TOPUP = webhook("evt_1", "topup.succeeded", "a", "25.00")

    def opened(self):
        service = self.service()
        service.open_wallet("a")
        return service

    def test_duplicate_after_restart(self):
        self.assertEqual(self.opened().handle_webhook(self.TOPUP), "applied")
        restarted = self.service()
        self.assertEqual(restarted.handle_webhook(self.TOPUP), "duplicate")
        self.assertEqual(restarted.balance("a"), Decimal("25.00"))
        self.assertEqual(restarted.last_seq, 2)

    def test_duplicate_after_snapshot_and_restart(self):
        service = self.opened()
        service.handle_webhook(self.TOPUP)
        service.deposit("a", "1")
        service.take_snapshot()
        for use_snapshot in (True, False):
            restarted = self.service(use_snapshot)
            self.assertEqual(restarted.handle_webhook(self.TOPUP), "duplicate")
            self.assertEqual(restarted.balance("a"), Decimal("26.00"))

    def test_duplicate_with_different_payload(self):
        service = self.opened()
        service.open_wallet("b")
        service.handle_webhook(self.TOPUP)
        retry = webhook("evt_1", "chargeback.created", "b", "99")
        self.assertEqual(service.handle_webhook(retry), "duplicate")
        self.assertEqual(self.service().handle_webhook(retry), "duplicate")
        self.assertEqual(self.service().balance("b"), Decimal(0))

    def test_duplicate_in_a_new_process(self):
        service = self.opened()
        service.handle_webhook(self.TOPUP)
        service.take_snapshot()
        code = ("import sys; from ledger import WalletService\n"
                f"s = WalletService(sys.argv[1])\nprint(s.handle_webhook({self.TOPUP!r}))\n"
                "s.take_snapshot()\nprint(s.balance('a'))\n")
        out = self.run_python(code).split()
        self.assertEqual(out[0], "duplicate")
        self.assertEqual(Decimal(out[1]), Decimal("25.00"))
        self.assertEqual(self.service().balance("a"), Decimal("25.00"))

    def test_failed_delivery_does_not_consume_key(self):
        service = self.opened()
        with self.assertRaises(InvalidAmount):
            service.handle_webhook(webhook("evt_2", "topup.succeeded", "a", "1.001"))
        with self.assertRaises(UnknownWallet):
            service.handle_webhook(webhook("evt_2", "topup.succeeded", "zed", "1"))
        self.assertEqual(self.service().handle_webhook(webhook("evt_2", "topup.succeeded", "a", "1")), "applied")
        self.assertEqual(self.service().balance("a"), Decimal("1"))

    def test_chargeback_can_overdraw(self):
        service = self.opened()
        self.assertEqual(service.handle_webhook(webhook("cb", "chargeback.created", "a", "5")), "applied")
        self.assertEqual(service.balance("a"), Decimal("-5"))
        with self.assertRaises(InsufficientFunds):
            service.withdraw("a", "0.01")
        self.assertEqual(self.service().balance("a"), Decimal("-5"))


class ReplayToolTest(LedgerCase):
    def test_replay_tool_modes_agree(self):
        self.at(2026, 3, 2, 9, 0, tz=TOKYO)
        service = self.service()
        service.open_wallet("b")
        service.open_wallet("a")
        self.at(2026, 3, 1, 19, 30, tz=NEW_YORK)
        service.deposit("a", "0.10")
        service.deposit("a", "0.20")
        service.take_snapshot()
        service.deposit("b", "12345678901234567.89")
        outputs = []
        for flags in ([], ["--from-scratch"], ["--verify"]):
            result = subprocess.run([sys.executable, "-m", "ledger.replay", self.data_dir, *flags],
                                    capture_output=True, text=True, timeout=60)
            self.assertEqual(result.returncode, 0, (flags, result.stderr))
            rows = [line.split() for line in result.stdout.splitlines()]
            outputs.append([(name, Decimal(value)) for name, value in rows])
        want = [("a", Decimal("0.30")), ("b", Decimal("12345678901234567.89")), ("last_seq", Decimal(5))]
        self.assertEqual(outputs, [want, want, want])


class RandomizedTest(LedgerCase):
    """Random commands, webhooks (with retries), snapshots, restarts and rebuilds."""

    SEEDS = range(40)
    STEPS = 140

    def amount(self, rng) -> str:
        roll = rng.random()
        if roll < 0.55:
            cents = rng.randint(1, 50_000)
        elif roll < 0.85:
            cents = rng.randint(1, 99)
        else:
            cents = rng.randint(10 ** 15, 10 ** 20)
        if cents % 100 == 0 and rng.random() < 0.5:
            return str(cents // 100)
        if cents % 10 == 0 and rng.random() < 0.5:
            return f"{cents // 100}.{(cents % 100) // 10}"
        return f"{cents // 100}.{cents % 100:02d}"

    def check(self, service, model, label):
        self.assertEqual(service.wallets(), sorted(model.entries), label)
        self.assertEqual(service.last_seq, model.last_seq, label)
        for wallet_id in model.entries:
            self.assertEqual(service.balance(wallet_id), model.balance(wallet_id), f"{label} balance {wallet_id}")
            for month in model.months():
                opening, credits, debits, closing, lines = model.statement(wallet_id, month)
                self.assert_statement(service.statement(wallet_id, month), opening, credits, debits,
                                      closing, lines, f"{label} statement {wallet_id} {month}")

    def step(self, rng, service, model, used_keys, spare_keys):
        roll = rng.random()
        wallets = sorted(model.entries)
        if not wallets or (roll < 0.08 and len(wallets) < 5):
            wallet_id = f"w{len(wallets)}"
            self.assertEqual(service.open_wallet(wallet_id), model.open(wallet_id))
            return service
        wallet_id = rng.choice(wallets)
        month = self.clock.month()
        if roll < 0.35:
            amount = self.amount(rng)
            self.assertEqual(service.deposit(wallet_id, amount), model.move(wallet_id, "deposit", amount, month))
        elif roll < 0.55:
            balance = model.balance(wallet_id)
            amount = f"{balance:f}" if balance > 0 and rng.random() < 0.2 else self.amount(rng)
            if Decimal(amount) > balance:
                with self.assertRaises(InsufficientFunds):
                    service.withdraw(wallet_id, amount)
            else:
                self.assertEqual(service.withdraw(wallet_id, amount), model.move(wallet_id, "withdrawal", amount, month))
        elif roll < 0.80:
            kind = "chargeback.created" if rng.random() < 0.3 else "topup.succeeded"
            amount = self.amount(rng)
            if used_keys and rng.random() < 0.4:
                key = rng.choice(sorted(used_keys))
                self.assertEqual(service.handle_webhook(webhook(key, kind, wallet_id, amount)), "duplicate")
            elif rng.random() < 0.1:
                key = f"bad{len(spare_keys)}-{model.last_seq}"
                with self.assertRaises(ValueError):
                    service.handle_webhook(webhook(key, kind, wallet_id, rng.choice(["1.001", "-1", "0", "NaN"])))
                spare_keys.add(key)
            else:
                key = spare_keys.pop() if spare_keys and rng.random() < 0.5 else f"evt-{model.last_seq}-{rng.randint(0, 999)}"
                self.assertEqual(service.handle_webhook(webhook(key, kind, wallet_id, amount)), "applied")
                model.move(wallet_id, "topup" if kind == "topup.succeeded" else "chargeback", amount, month)
                used_keys.add(key)
        elif roll < 0.88:
            self.assertEqual(service.take_snapshot(), model.last_seq)
        elif roll < 0.96:
            service = self.service(use_snapshot=rng.random() < 0.75)
        else:
            service.rebuild_projections()
        return service

    def run_seed(self, seed):
        rng = random.Random(seed)
        model, used_keys, spare_keys = Model(), set(), set()
        self.clock.instant = datetime(2026, 1, 30, 20, 0, tzinfo=UTC)
        service = self.service()
        for i in range(self.STEPS):
            self.clock.instant += timedelta(minutes=rng.randint(1, 60 * 20))
            self.clock.offset = rng.choice(OFFSETS)
            service = self.step(rng, service, model, used_keys, spare_keys)
            if i % 35 == 34:
                self.check(service, model, f"seed {seed} step {i}")
        self.check(service, model, f"seed {seed} live")
        self.check(self.service(use_snapshot=False), model, f"seed {seed} from scratch")
        self.check(self.service(use_snapshot=True), model, f"seed {seed} snapshot + tail")
        restarted = self.service()
        restarted.rebuild_projections()
        self.check(restarted, model, f"seed {seed} rebuilt")
        for key in sorted(used_keys)[:5]:
            self.assertEqual(restarted.handle_webhook(webhook(key, "topup.succeeded", "w0", "1")), "duplicate")
        self.assertEqual(restarted.last_seq, model.last_seq)

    def test_random_sequences(self):
        for seed in self.SEEDS:
            with self.subTest(seed=seed), tempfile.TemporaryDirectory() as data_dir:
                self.data_dir = data_dir
                self.run_seed(seed)


if __name__ == "__main__":
    unittest.main()
