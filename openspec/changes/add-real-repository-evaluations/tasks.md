## 1. Ordered goal and design review

- [x] 1.1 Verify PR #48 merged, final head `6896a7b3` passed all 16 CI jobs, and merged main `e295f43d` passed CI; pull main.
- [x] 1.2 Inspect existing eval/solve contracts and reviewed reserved-budget/scheduling/retention designs; prepare proposal, design and tasks.
- [x] 1.3 Review these new benchmark artifacts with the user before implementation. Approved by the user's “ok so continue” on 2026-10-26 after the artifact-review request and clarification.

## 2. Real source corpus and controls

- [x] 2.1 Pass bounded disk/RAM/toolchain preflight; reclaim only owned regenerable build caches if necessary.
- [ ] 2.2 Select four real historical tasks across two repositories (at least two Rust); freeze source, license, prompt, setup/dependency and grader provenance before live calls.
- [x] 2.3 Implement pinned import/materialization with verified tree identity and base-only fresh Git history, retaining complete usable source and notices.
- [x] 2.4 Add independent graders/reference fixes; prove base FAIL/reference PASS, upstream regression checks and test-tampering refusal without modifying graded source.
- [x] 2.5 Validate backward-compatible manifests/provenance, setup failure classification, path boundaries and cache behavior with offline tests.

## 3. Separate single-agent and solve evaluation

- [x] 3.1 Add explicit strategy selection retaining current default behavior; invoke bundled solve with two candidates and a separate judge.
- [x] 3.2 Bind final grading to authoritative selected-source evidence; reject missing recommendations, invented workflow evidence and post-check source changes.
- [ ] 3.3 Record complete strategy-specific costs/tokens/uncertainty, check/judge outcomes, failure denominators and actual elapsed time; test partial/infrastructure/timeout outcomes.
- [ ] 3.4 Freeze trial order, model, shared account/store, budgets and environment; verify no personal hooks/config, grader feedback or future Git objects leak into attempts.
- [ ] 3.5 Run twelve single trials and twelve separate solve runs, three per task; retain every outcome and separately label any diagnostics.
- [ ] 3.6 Publish reviewed reports, reproducible provenance, limitations and comparisons while verifying original pilot artifacts remain byte-identical.
- [ ] 3.7 Run relevant Rust tests, format/Clippy, grader controls, strict OpenSpec, documentation checks and substantive code review; deliver green relevant CI and reviewable PRs.

## 4. Reserved budgets (existing approved scope)

- [ ] 4.1 Implement/verify original `complete-dashboard-and-automation` tasks 4.1–4.5, including all provider/auxiliary paths, concurrent and nested scope accounting, uncertainty and audited reconciliation.
- [ ] 4.2 Document priced/unpriced admission and token/time limitations; synchronize original tasks, tests, APIs/SDK and user evidence, and deliver a separate reviewed PR with green CI.

## 5. Persistent schedules and transcript retention (existing approved scope)

- [ ] 5.1 Implement/verify original schedule tasks 6.1–6.6: cron/one-shot, timezone/DST, durable occurrence claims, overlap/missed policy, trust/permissions/budgets, hooks and CLI/API/SDK surfaces.
- [ ] 5.2 Implement/verify original retention tasks 5.1–5.4: configurable expiry, transactional live/replay/evidence protection, tombstones and retained summaries/costs/branches.
- [ ] 5.3 Synchronize original tasks and documentation; deliver reviewable increments with meaningful restart/race regressions and green relevant CI.
- [ ] 5.4 Review merged/main evidence and archive completed changes only after acceptance; report any unperformed checks without claiming them passed.
