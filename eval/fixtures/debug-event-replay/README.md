# ledger

An event-sourced wallet service (Python 3.10+, standard library only). Every change to a
wallet is an immutable event appended to a JSON Lines log; balances, statements and the
webhook de-duplication state are all derived from that log.

## Modules

| Module | Responsibility |
|---|---|
| `ledger/errors.py` | Exception types (`LedgerError` and subclasses). |
| `ledger/money.py` | Parsing and validating amounts (`decimal.Decimal`). |
| `ledger/clock.py` | Clocks and event timestamps. |
| `ledger/events.py` | The `Event` record, event types and the JSON Lines encoding. |
| `ledger/store.py` | `EventStore`: the append-only log `events.jsonl`. |
| `ledger/wallet.py` | The `Wallet` aggregate and `WalletBook` (all wallets, rebuilt by applying events). |
| `ledger/commands.py` | Command handlers: validate a command against the current state and produce the event to append. |
| `ledger/snapshots.py` | Snapshots of the wallet state (`snapshot.json`). |
| `ledger/projections.py` | The monthly statement projection. |
| `ledger/webhooks.py` | Payment-provider webhook ingestion with idempotency keys. |
| `ledger/service.py` | `WalletService`, the facade used by the API layer and the tools. |
| `ledger/replay.py` | `python3 -m ledger.replay`, the operator's replay/verification tool. |

## Public API

```python
from ledger import WalletService

service = WalletService(data_dir, clock=clock, use_snapshot=True)
```

* `data_dir` is a directory (created if missing) holding `events.jsonl` and `snapshot.json`.
  Any number of `WalletService` objects may be created over the same directory one after the
  other (a process restart is simply a new `WalletService` on the same directory, in the same
  or in a new process); only one is used at a time.
* `clock` is a zero-argument callable returning a timezone-aware `datetime`; it defaults to the
  host's clock in the host's local UTC offset. The service calls it once for each event it
  appends, and that value (with its UTC offset, as `datetime.isoformat()` renders it) becomes
  the event's `recorded_at`. Hosts run in different regions, so `recorded_at` values in one log
  carry **different UTC offsets**. A clock returning a naive datetime raises `ValueError`.
* `use_snapshot=True` starts from the latest snapshot (if any) and replays only the events
  after it; `use_snapshot=False` replays the whole log from scratch.

Commands (each appends exactly one event and returns its sequence number):

* `open_wallet(wallet_id)` - ids are 1-64 characters from `A-Z a-z 0-9 _ -`
  (`ValueError` otherwise); an existing id raises `WalletExists`.
* `deposit(wallet_id, amount)` - adds `amount`.
* `withdraw(wallet_id, amount)` - subtracts `amount`; raises `InsufficientFunds` when the
  balance is lower than `amount` (a balance equal to the amount may be withdrawn).
* `handle_webhook(payload)` - see *Webhooks*; returns `"applied"` or `"duplicate"`
  (a duplicate appends nothing).

A command that raises appends nothing. Unknown wallet ids raise `UnknownWallet` (also for
`balance` and `statement`).

Queries:

* `balance(wallet_id) -> Decimal`
* `wallets() -> list[str]` - all wallet ids, sorted.
* `last_seq -> int` - sequence number of the last event in the log (0 for an empty log).
* `statement(wallet_id, month) -> Statement` - see *Statements*.

Maintenance:

* `take_snapshot() -> int` - writes `snapshot.json` covering every event in the log and
  returns the sequence number of the last event it covers (0 for an empty log).
* `rebuild_projections()` - discards all projection state and rebuilds it from the full log
  (run nightly by ops, and after deploying projection changes).

## Amounts

Amounts are passed as decimal strings (or `Decimal`), must be finite, greater than zero and
have at most two fractional digits (`"0.01"`, `"12.5"`, `"7"`); anything else raises
`InvalidAmount` (a `ValueError`). There is no upper limit (`"12345678901234567.89"` is a valid
amount) and all arithmetic is exact: a balance is always exactly the sum of its credits minus
its debits, as a `Decimal`, no matter how the state was reconstructed.

## Events and sequence numbers

The log assigns sequence numbers 1, 2, 3, ... in append order. Events are **always applied in
sequence-number order** - when replaying, in projections and anywhere else. `recorded_at` is
informational (and used for statement months); it is never used to order events.

## Snapshots

A snapshot stores the state of every wallet after a given event so that startup only has to
replay the tail of the log. The amounts in a snapshot must round-trip exactly.

## Webhooks

Payment providers deliver events as dicts:

```python
{"idempotency_key": "evt_123", "type": "topup.succeeded", "wallet_id": "alice", "amount": "25.00"}
```

* `type` is `"topup.succeeded"` (credits the wallet, event `TopUpReceived`) or
  `"chargeback.created"` (debits the wallet, event `ChargebackReceived`; a chargeback is
  applied even if it makes the balance negative).
* `idempotency_key` is a non-empty string. Providers retry deliveries (minutes or days later),
  so **a delivery whose key has already been applied is a duplicate**: it returns
  `"duplicate"` and changes nothing, whatever the rest of its payload says. This holds across
  restarts, snapshots and processes: the key is recorded in the event it produced, and the
  log is the source of truth.
* A delivery that raises (unknown type, unknown wallet, invalid amount, missing key) is not
  applied and does not consume its key.

## Statements

`statement(wallet_id, "YYYY-MM")` returns a `Statement(wallet_id, month, opening, credits,
debits, closing, lines)`. An event belongs to the **UTC** calendar month of its `recorded_at`
(an event recorded at `2026-04-01T05:00:00+09:00` belongs to `2026-03`).

* `lines` is a tuple of `StatementLine(seq, kind, amount, balance)` for the wallet's events of
  that month in sequence-number order; `kind` is `"deposit"`, `"withdrawal"`, `"topup"` or
  `"chargeback"`, `amount` is positive, `balance` is the running balance after the line.
* `opening` is the sum of the wallet's credits minus debits in all earlier months, `credits`
  and `debits` are the totals (positive) of the month, `closing = opening + credits - debits`,
  and the running balances start from `opening`.
* A month without events has no lines and `closing == opening`. A month that is not
  `YYYY-MM` (with `MM` from 01 to 12) raises `ValueError`. The statement of the month
  of a wallet's last event closes at its `balance()`.

Statements are a projection: rebuilding them (`rebuild_projections()`, or a new service over
the same log) must always give exactly the same result.

## Invariants

For any sequence of commands, snapshots, restarts and rebuilds:

1. live state == replay from scratch (`use_snapshot=False`) == replay from the latest
   snapshot plus the tail (`use_snapshot=True`), for every balance and every statement;
2. a webhook idempotency key is applied at most once;
3. events are applied in sequence-number order.

## Replay tool

```sh
python3 -m ledger.replay DATA_DIR             # balances from snapshot + tail
python3 -m ledger.replay DATA_DIR --from-scratch
python3 -m ledger.replay DATA_DIR --verify    # exit 1 if the two disagree
```

In every mode it prints one `<wallet_id> <balance>` line per wallet (sorted by id), then
`last_seq <n>`; `--verify` prints the snapshot + tail result.

## Tests

```sh
python3 -m unittest discover -s tests
```
