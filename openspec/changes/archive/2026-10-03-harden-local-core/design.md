# Design

## Decisions

The local server remains the execution and database writer owner. SQLite WAL with FULL synchronous is the local persistence boundary. PostgreSQL is reserved for shared hosted control-plane state; see `docs/decisions/0001-storage-architecture.md` for alternatives, limits and measurements.

Tool dispatch records intent before execution, but cannot atomically commit arbitrary external effects. Recovery therefore distinguishes known failure from unknown outcome. Retrying an uncertain mutation requires reconciliation or explicit user authorization. Equivalent new calls cannot bypass the hold.

Repository-controlled executable configuration is inactive until approved for that checkout and definition digest. Trust and saved approvals cannot bypass explicit user ceilings, plan mode or the sandbox. Basic enforcement ships with the local core on supported platforms.

Rewind performs conflict-aware restoration and preserves current bytes when it cannot safely apply an inverse change. Compaction retains a durable task-state record and source IDs separately from generated prose. Earlier Session context remains searchable without granting access to unrelated Sessions.

Budget behavior is explicit: soft limits disclose overshoot; reserved limits reserve bounded estimated charges atomically and retain uncertainty for interrupted requests. Remote transfer uses durable ownership epochs and a fail-closed transfer protocol.

## Validation

Run OpenSpec structural validation, compare modified requirements against the prior target, regenerate phase counts, and inspect cross-spec contracts. Runtime assertions and live-provider evaluation requirements are delivery obligations, not tests executable in this specification-only folder.

## Tradeoffs

FULL synchronous can increase write latency; short transactions and measured workload gates determine whether batching is needed. Trust and unknown-outcome holds can require user attention, but make unattended behavior explicit. Native provider breadth and advanced orchestration follow a validated local core. Browser automation is optional and uses an isolated supported integration.
