## 1. Review and shared library foundation

- [ ] 1.1 Review proposal, design, delta scenarios and the clean-checkout/soft-budget limits before runtime implementation.
- [ ] 1.2 Implement or reuse M2.2 sections 1–4: embedded origin, discovery/overrides, args schemas, library HTTP/CLI/SDK, approvals and bounded results. Ship only implemented modules in listings.
- [ ] 1.3 Add `solve` schema/defaults, model resolution and captured base identity; reject invalid inputs, missing models and dirty/non-Git bases before any child starts.

## 2. Isolated candidates

- [ ] 2.1 Launch two to four independent child Sessions on managed worktrees from the recorded base, using existing scheduling and ceilings.
- [ ] 2.2 Record candidate authority, branch, final source/diff digest and settled owned execution before verification.
- [ ] 2.3 Production-path tests: same base/different models, same-model disclosure, setup failure, denied edit/command and no owner/sibling file mutation.

## 3. Host verification

- [ ] 3.1 Implement host-only permission/hook/sandbox-aware checks, source fencing, bounded logs and the seven check outcomes.
- [ ] 3.2 Add durable records/schema migration as required; reuse only matching settled evidence and fence unknown native effects.
- [ ] 3.3 Regression tests: agent says "passed" but exits nonzero; check mutates source; denied command never launches; timeout/cancellation/unknown is not success; truncated logs remain labelled; restart never duplicates an unresolved effect.

## 4. Judge and report

- [ ] 4.1 New read-only judge Session with the documented rubric, untrusted-data framing and structured output validation.
- [ ] 4.2 Enforce eligibility outside model output, reject an ineligible selection and permit no recommendation.
- [ ] 4.3 Persist/report candidates, checks, usage, limits, judge and nullable recommendation; return partial evidence on budget/interrupt.
- [ ] 4.4 Tests: a project `solve` override cannot fabricate host-evidence badges; good/bad/deceptive candidates, all fail, judge unavailable, same judge model, budget overshoot disclosure and no automatic merge/publication.

## 5. Client surfaces and acceptance

- [ ] 5.1 CLI/API/SDK round-trip of the report and artifact links, existing exit semantics, library args form and TUI evidence view.
- [ ] 5.2 Real-binary scripted acceptance: three candidates, one rejected by checks, recommendation with inspectable evidence, restart during solving/checking/judging and budget stop.
- [ ] 5.3 Opt-in live multi-model acceptance with exact providers/models/versions, budgets, outcomes and failures retained; no quality-parity claim.
- [ ] 5.4 Update feature/usage docs and M2.2 status only after implementation; archive only after task/evidence review.
