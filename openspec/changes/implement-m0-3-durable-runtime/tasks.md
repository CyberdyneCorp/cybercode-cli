## 1. State and storage

- [x] 1.1 Session event payloads, registry and SQL projectors; sessions migration.
- [x] 1.2 State fold with replay; optimistic-concurrency commits; public store read API.

## 2. Admission and Drains

- [x] 2.1 Idempotent admission, steer/queue/hold, inbox edit, remove, release and refuse.
- [x] 2.2 Drain coordinator, coalesced wakes, forced resume, Safe Boundary order.
- [x] 2.3 Turn assembly, streaming persistence, live events, usage and cost, reasoning lowering.

## 3. Tools and recovery

- [x] 3.1 Tool host contract, dispatch records, unknown, stale, invalid and repaired calls, parallel groups, step limit.
- [x] 3.2 Interrupt settlement, crash recovery with reconciliation, unknown-outcome pause and resolution, crash redaction.
- [x] 3.3 Panic-safe Drains.

## 4. Context and compaction

- [x] 4.1 Context sources, epochs, reconciliation, blocked initialization, provider-family switch, repair.
- [x] 4.2 Automatic, overflow and manual compaction; tail; task state; media stripping; failure; auto-continue.

## 5. Session management and verification

- [x] 5.1 Titles, rename, archive, fork, cascading delete, list paging, switches.
- [x] 5.2 Runtime, context and compaction tests; regression tests; live example; recovery manifest cases.
- [x] 5.3 Spec clarification; README and ROADMAP status.
