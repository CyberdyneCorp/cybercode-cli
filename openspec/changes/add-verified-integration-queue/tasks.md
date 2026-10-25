## 1. Review

- [x] 1.1 Inspect solve, hub, worktree and workflow contracts; identify missing combined-contribution verification.
- [x] 1.2 Draft proposal, design and testable contract deltas.
- [x] 1.3 Review artifacts with the user and record approval before capability implementation. On 2026-10-24 the user explicitly answered “Approve both designs and implement” after receiving the integration-queue and macOS computer-use design links in PR #32.

## 2. Domain and durable runtime

- [x] 2.1 Add bounded batch/attempt manifests, `intg_` identities, contribution dependencies, deterministic ordering and revision-fenced queue storage/migrations.
- [x] 2.2 Add owner-authorized submission, repository/commit validation and optional current hub attribution without an outage bypass.
- [x] 2.3 Compose changes in managed isolated worktrees with conflict evidence and safe Git plumbing; never alter the owner's checkout/target.
- [x] 2.4 Run pinned host-owned verification on the combined source, protect commands/environment from contributor replacement and detect post-check mutation.
- [x] 2.5 Add independent read-only review and derived evidence/report status with actual/estimated/unpriced cost.
- [x] 2.6 Add cancellation, durable intents/settlements, unknown outcomes, target staleness, replay/idempotency and explicit rerun attempts.

## 3. Surfaces and explicit publication

- [x] 3.1 Add workflow integration CLI operations, typed API/OpenAPI and generated SDK operations.
- [x] 3.2 Add dashboard queue, manifest/diff, dependencies/conflicts, checks, review and cost views with immutable action targets.
- [x] 3.3 Add explicit owner-reviewed branch/PR publication; persist intent, reconcile lost acknowledgements and prohibit automatic merge/force-push.

## 4. Verification and docs

- [x] 4.1 Regression: two patches pass alone and fail together; final combined check status must fail.
- [x] 4.2 Cover textual conflicts, dependencies/cycles, unrelated repositories, malicious hooks/config, stale branch tips, source mutation and unavailable reviewer.
- [x] 4.3 Cover queue races, restart during checking, uncertain native/publication outcomes, budgets and cancellation without duplicate effects.
- [x] 4.4 Run actual-server CLI/SDK/browser acceptance, privacy and current hub authorization tests; report live Auth outage separately.
- [ ] 4.5 Update README, roadmap, usage/evidence docs and retention references; validate specs/generated artifacts and relevant platform CI; perform code review before ready-to-merge.

## Acceptance status

Local implementation evidence and deployment limits are recorded in [verified-integration-acceptance.md](../../../docs/measurements/verified-integration-acceptance.md). Live CyberdyneAuth acceptance remains blocked by the external outage; hub-linked operations fail closed. Real GitHub publication and non-macOS platform acceptance remain deployment/CI checks, not fixture-proven claims. Required `integrate` transcripts must remain excluded when the parallel retention implementation is integrated.
