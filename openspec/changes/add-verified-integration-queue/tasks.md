## 1. Review

- [x] 1.1 Inspect solve, hub, worktree and workflow contracts; identify missing combined-contribution verification.
- [x] 1.2 Draft proposal, design and testable contract deltas.
- [ ] 1.3 Review artifacts with the user and record approval before capability implementation.

## 2. Domain and durable runtime

- [ ] 2.1 Add bounded batch/attempt manifests, `intg_` identities, contribution dependencies, deterministic ordering and revision-fenced queue storage/migrations.
- [ ] 2.2 Add owner-authorized submission, repository/commit validation and optional current hub attribution without an outage bypass.
- [ ] 2.3 Compose changes in managed isolated worktrees with conflict evidence and safe Git plumbing; never alter the owner's checkout/target.
- [ ] 2.4 Run pinned host-owned verification on the combined source, protect commands/environment from contributor replacement and detect post-check mutation.
- [ ] 2.5 Add independent read-only review and derived evidence/report status with actual/estimated/unpriced cost.
- [ ] 2.6 Add cancellation, durable intents/settlements, unknown outcomes, target staleness, replay/idempotency and explicit rerun attempts.

## 3. Surfaces and explicit publication

- [ ] 3.1 Add workflow integration CLI operations, typed API/OpenAPI and generated SDK operations.
- [ ] 3.2 Add dashboard queue, manifest/diff, dependencies/conflicts, checks, review and cost views with immutable action targets.
- [ ] 3.3 Add explicit owner-reviewed branch/PR publication; persist intent, reconcile lost acknowledgements and prohibit automatic merge/force-push.

## 4. Verification and docs

- [ ] 4.1 Regression: two patches pass alone and fail together; final combined check status must fail.
- [ ] 4.2 Cover textual conflicts, dependencies/cycles, unrelated repositories, malicious hooks/config, stale branch tips, source mutation and unavailable reviewer.
- [ ] 4.3 Cover queue races, restart during checking, uncertain native/publication outcomes, budgets and cancellation without duplicate effects.
- [ ] 4.4 Run actual-server CLI/SDK/browser acceptance, privacy and current hub authorization tests; report live Auth outage separately.
- [ ] 4.5 Update README, roadmap, usage/evidence docs and retention references; validate specs/generated artifacts and relevant platform CI; perform code review before ready-to-merge.
