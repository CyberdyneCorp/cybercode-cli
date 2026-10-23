## 1. Review and shared library foundation

- [x] 1.1 Review proposal, design, delta scenarios and the clean-checkout/soft-budget limits before runtime implementation.
- [x] 1.2 Implement or reuse M2.2 sections 1–4: embedded origin, discovery/overrides, args schemas, library HTTP/CLI/SDK, approvals and bounded results. Ship only implemented modules in listings. (The foundation exists: `implement-m2-2-bundled-workflows` shipped it with the five original modules.)
- [x] 1.3 Add `solve` schema/defaults, model resolution and captured base identity; reject invalid inputs, missing models and dirty/non-Git bases before any child starts.

## 2. Isolated candidates

- [x] 2.1 Launch two to four independent child Sessions on managed worktrees from the recorded base, using existing scheduling and ceilings.
- [x] 2.2 Record candidate authority, branch, final source/diff digest and settled owned execution before verification.
- [x] 2.3 Production-path tests: same base/different models, same-model disclosure, setup failure, denied edit/command and no owner/sibling file mutation.

## 3. Host verification

- [x] 3.1 Implement host-only permission/hook/sandbox-aware checks, source fencing, bounded logs and the seven check outcomes.
- [x] 3.2 Add durable records/schema migration as required; reuse only matching settled evidence and fence unknown native effects.
- [x] 3.3 Regression tests: agent says "passed" but exits nonzero; check mutates source; denied command never launches; timeout/cancellation/unknown is not success; truncated logs remain labelled; restart never duplicates an unresolved effect.

## 4. Judge and report

- [x] 4.1 New read-only judge Session with the documented rubric, untrusted-data framing and structured output validation.
- [x] 4.2 Enforce eligibility outside model output, reject an ineligible selection and permit no recommendation.
- [x] 4.3 Persist/report candidates, checks, usage, limits, judge and nullable recommendation; return partial evidence on budget/interrupt.
- [x] 4.4 Tests: a project `solve` override cannot fabricate host-evidence badges; good/bad/deceptive candidates, all fail, judge unavailable, same judge model, budget overshoot disclosure and no automatic merge/publication.

## 5. Client surfaces and acceptance

- [ ] 5.1 CLI/API/SDK round-trip of the report and artifact links, existing exit semantics, library args form and TUI evidence view.
- [ ] 5.2 Real-binary scripted acceptance: three candidates, one rejected by checks, recommendation with inspectable evidence, restart during solving/checking/judging and budget stop.
- [ ] 5.3 Opt-in live multi-model acceptance with exact providers/models/versions, budgets, outcomes and failures retained; no quality-parity claim.
- [ ] 5.4 Update feature/usage docs and M2.2 status only after implementation; archive only after task/evidence review.

## Admission increment (2026-10-23)

The shared M2.2 library shipped in PR #18. This increment implements only preflight admission: `cyber-workflow::solve::Args` validates/defaults the task, candidate refs and check argv; the existing workflow-start preparation path accepts `meta.prepare: "solve"`, resolves every solver/judge model, enforces explicit read denials and uses sandboxed Git metadata plus the external-filter-free libgit2 helper to record a clean HEAD/root/common-dir snapshot. Admission data is not host-verification evidence. It does not launch candidates, run checks, judge patches or register a bundled `solve` entry.

Evidence: four parser unit tests plus five `cyber-app` production HTTP cases in `tests/support/solve_admission.rs`. The Git integration cases run on Unix; native Windows admission remains unverified. Candidate checkout creation must use/revalidate this pinned base in section 2; section 3 must fence writes and settle native checks before any verified result is offered.

Regression evidence: a same-size edit under a repository clean filter reproduced a write to the owner checkout with native `git status`; the filter test failed before switching status inspection to the sandboxed libgit2 helper and passes after it.

Validation of the admission increment: `cargo test --locked -p cyber-workflow` (68 passed), `cargo test --locked -p cyber-app --test workflows` (48 passed), affected-crate all-target Clippy with warnings denied, formatting, generated SDK consistency and strict OpenSpec validation (85 items) pass. Cross-spec lint reports 0 errors and 23 warnings. This is local macOS evidence; the full workspace and native Linux/Windows execution were not rerun.

## Runtime implementation evidence (2026-10-23)

Implemented in the PR #23 branch: the sixth embedded module, immutable candidate bases, 20-step solver ceilings, source/subtree fencing, literal-argv native checks, durable snapshot/intent/settlement aggregates, bounded logs and explicit unknown effects, read/glob/grep-only Plan-mode judging, eligibility validation, authoritative API/SDK/terminal evidence and usage documentation.

Recovery regression: a stopped isolated agent whose child does not yet exist must retain its original host-generated worktree plan. The deterministic runtime regression failed before the fix (resume supplied no worktree) and passes after restoring the recorded branch ordinal and pinned base; missing or mismatched isolation metadata fails closed. This also preserves the original isolation choice for ordinary workflows.

Local production tests cover separate candidate patches, host rejection of false solver claims, source mutation, command denial, full-access/extra-root and direct file-tool escape attempts, queued repository replacement, setup failure, denied edits, same-model disclosure, restarts during solving/checking/judging, interactive approval after submission, bounded/truncated logs, timeout and detached-descendant unknown settlement with no subsequent native dispatch. The real CLI acceptance compares three candidates across two model identities, rejects one and recommends a passing patch while preserving the owner checkout. Parser, engine and source-fingerprint regressions cover malformed admission, ineligible judge selections, budget partials, binary source, symlinks and deleted directories. SDK authority/artifact round-trip and terminal snapshots are included.

Validation in progress: final tests after main integration, generated API consistency, full workspace checks and platform CI. Live paid-model acceptance (5.3) is opt-in and has not been run; this change makes no model-quality parity claim. Do not archive or mark the remaining client/acceptance tasks complete without final verification.
