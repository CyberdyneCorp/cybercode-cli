# Reserved budget implementation evidence (2026-10-26)

The implementation adds durable atomic request allocations, cumulative receipt deltas and owner reconciliation. All applicable Session/run/Goal/Loop/Team/daily scopes and scheduled-occurrence bindings share one admission transaction. Token/cost holds remain independent across retry attempts; one canonical Session/message turn claim is shared. Financial receipts commit before capacity is freed; canonical transcript fields remain available with an accounting marker.

Initial production support is official stateless text/function OpenAI Responses with declared model context, transmitted output cap and conservative catalog prices. Other production adapters explicitly refuse. This is scheduling against declared metadata, not an invoice guarantee. ChatGPT subscription token/time controls remain soft. Operational examples and limitations are in [Reserved budgets](../reserved-budgets.md).

Local checks passed:

| Check | Result |
| --- | --- |
| `cargo test -p cyber-store --test reservations` | 15 passed: atomic concurrent admission, rollback, request identity, uncertain restart, cumulative settlement, owner audit, deadlines and canonical retry holds |
| `cargo test -p cyber-server --lib runtime::reservations` | 12 passed: inherited/current scopes, missing evidence, all five token classes, reserved receipt-to-canonical turn interleaving, local-midnight partial/unknown/audited settlement and saved scheduled context |
| Focused native usage folding and signed child receipt tests | 2 passed: linked evidence retained, exact deltas, replay and underflow refusal |
| `cargo test -p cyber-server --test reserved_budgets` | 9 passed: no-dispatch bounds/pricing, concurrent children, canonical evidence/replay, post-Finish failure, cancellation during opening, deadline/retry turn identity and immutable channel/MCP attribution across queued inputs and late settlement |
| Existing `budgets` and `budget_errors` suites | 13 + 3 passed |
| HTTP reservation route tests and OpenAPI snapshot | 4 + 1 passed: owner auth, Origin, input envelopes, audit/idempotency, revision fencing and typed evidence |
| `cargo test -p cyber-core --lib budget::tests` | 2 passed |
| Reserved prompt-hook and web-summary tests | 2 + 1 passed; owned guard survives opening/consumption cancellation and avoids legacy double billing |
| `cargo test -p cyber-workflow --test bundled solve_` | 5 passed; report distinguishes reserved estimates from default soft ceiling |
| OpenAI Responses unit tests and request-price unit test | 5 + 1 passed, including bundled `gpt-4.1` catalog metadata offline and absent/malformed terminal usage |
| Standalone Git text reserved-daily refusal | 1 passed, before model resolution/network access |
| SDK typecheck/tests and generation consistency | Passed; 117 tests passed, 9 opt-in process acceptance tests skipped |
| Dashboard unit tests/typecheck/embedded build | 46 passed, typecheck/build passed; delayed allocation evidence cannot cross Session switches |
| Strict OpenSpec change validation | Passed |

The code-review pass identified and corrected missing usage being interpreted as zero, retry turn duplication, the settlement-to-canonical-event turn gap, predispatch deadline cleanup, and masked receipt flush errors. All production dispatch paths found in the server, tool host and standalone Git text service are guarded or explicitly refused where no Session accounting owner exists.

Pending integration checks: combined retention/schedule branch acceptance with real occurrence promotion and late auxiliary launch; dashboard real-server browser acceptance for the new read-only allocation panel; broader relevant Rust suites and Linux/Windows CI. No live provider/billing, CyberdyneAuth or native desktop acceptance was run for this change. The SDK's skipped tests were not counted as passes.

Known limits: raw linked usage receipts remain retained rather than folding away their identity; historical opaque/folded failed accounting fails closed. Full Session deletion after explicit owner release prevents a later public native-accounting correction, although reservation audit records survive. Reserved Team ceilings include the lead. The original soft-mode partial-usage gaps are not retroactively reconstructed.

A follow-up review corrected reservation receipt attribution and cumulative-receipt analytics: provider calls retain their original input origin/MCP client across later foreground input and owner settlement; successful request/Turn counters count each reservation once while token/cost deltas remain additive. Confirmed priced partial receipts remain visible in the per-model cost breakdown even before a finished call exists.

Follow-up verification: reserved-budget integration tests 9/9 and usage-ledger migration tests 6/6 passed. Strict Clippy passed for cyber-server, cyber-store and cyber-tools across all targets; formatting and diff whitespace checks passed. Combined auto-fix branch verification remains separate.
