# Design

## Existing contracts

`cyber eval run` supports versioned manifests, disposable fixture trees, native budgets and independent hidden graders. It currently drives a single Session. `solve` already runs two to four candidates in managed worktrees, records host checks against exact source identities, and uses a read-only judge. The existing `harness-evaluation` specification requires repeated live trials and retained failure denominators. This change composes these paths rather than accepting model-reported tests or replacing the original benchmark.

## 1. Corpus and provenance

Freeze four tasks across two real public repositories, at least two in Rust. Initial sources are ripgrep and Click. Prefer bounded behavioral regressions with clear upstream intent over benchmarks whose only distinction is performance timing. A task must reproduce a real historical problem on an exact base commit and have a validated reference fix. If a proposed task is ambiguous, unbuildable or leaks its answer through visible files, replace it before freezing the pilot; publish the exclusion and reason.

Retain the complete source snapshot needed for the repository's ordinary build/tests, licenses and notices. Record repository URL, full base/fix SHAs, source-tree hash, reference change, prompt, visible verification commands, hidden grader hash and toolchain/dependency locks. Base/fix attribution and reference solutions live outside the candidate root. Materialize a fresh Git repository from the base snapshot only: do not expose later Git objects, remotes, hidden tests or reference patches to candidates.

Use host-owned setup to fetch and verify pinned sources and locked dependencies before live attempts. Agents and grading commands run with network off and in disposable bounded writable roots, without personal hooks/config or credential environment variables. Credential stores needed by the provider stay owned by the harness. Avoid repository-defined setup hooks without explicit review. The final source is compiled/executed only through the existing command isolation boundaries. Do not weaken sandboxing for a benchmark.

Maintain backward compatibility for existing manifests and reports. Add optional repository provenance and strategy metadata without changing existing field meanings. Keep downloaded source/dependency caches outside Git; manifests and import recipes must reconstruct and verify them. Validate imported paths, source identity and setup results before model dispatch. An unavailable prerequisite is an explicit setup failure, never a synthetic repository substituted under the same task ID.

## 2. Independent graders

Before freezing a task, prove the untouched base fails its behavioral regression and the reference fix passes both that regression and relevant upstream tests. Pin the independent tests outside all model-writable roots. Test that deleting visible tests or tampering with candidate test/config files cannot manufacture a pass. Independently build and exercise the final candidate, including through the actual Rust binary/library where applicable.

Reference solutions and grader feedback must never enter candidate or judge context. No live candidate is tuned or retried after seeing hidden results. Keep model quality, setup failures, provider interruptions, permission failures and timeouts distinguishable. A baseline that does not build cannot become a coding pass/fail observation.

## 3. Strategies and trial protocol

Run three trials per task per strategy: twelve single-agent trials and twelve separately reported solve runs. Use the existing `cyber eval run` path for single-agent execution; add explicit `--strategy single|solve` selection (default `single`) with strategy-specific report metadata. Existing invocations retain their behavior.

For solve, invoke the bundled immutable workflow through the existing embedded runtime. Start two candidate Sessions in independent clean worktrees and a separate read-only judge. Consume authoritative `solve_evidence`, not arbitrary workflow result JSON. The judge sees visible host verification evidence and patches, never the hidden grader. After selection, independently grade the exact selected source. No recommended candidate means the run does not pass, even if some unselected candidate passes a later diagnostic grade. Report candidate/selection/check/judge outcomes separately; a workflow lifecycle `completed` is not success on the task. Never auto-merge candidates or modify the source checkout.

Pin the requested model `chatgpt/gpt-6.1-sol#medium` for the initial single-agent, solver and judge roles; record resolved metadata, timestamp and any backend uncertainty. Use the same existing account/store and refresh locks for both strategies. Isolate personal configuration and avoid concurrent unrelated benchmark jobs. Run at most one root trial at a time, with two solvers inside each solve run. Predetermine and record a trial/strategy alternation order to reduce time-of-day bias.

Freeze budgets before the first live call. Initial limits per single trial/combined solve run: 1,500,000 accounted tokens and 1,800 seconds; single-agent 60 main steps, solve 20 steps per solver with the workflow's bounded agent count. Record all solver/judge/auxiliary usage and missing attribution. The token and step policies are scheduling limits, with current soft-accounting uncertainty disclosed. Do not compare these as identical internal execution caps. Model subscription pricing is unknown: do not claim a strict $5 guarantee or treat native zero-dollar fields as free execution. Reserved enforcement is priority 3 and not silently assumed implemented here.

An infrastructure failure stays in the denominator. Permit only explicitly labelled extra diagnostics after the original attempt settles; never replace a scored attempt. Stop at the declared limits; do not raise limits to obtain passing results. Live model tests are opt-in and are not ordinary PR CI jobs.

## 4. Reports and acceptance

Write separate single and solve reports, joined by task/trial identity only for paired analysis. Include all 24 root outcomes, native tokens and known attribution gaps, unpriced amounts, wall time, model/harness/environment hashes, trial order and independent grade results. Solve reports additionally link candidate source identities, host check receipts and recommendation. Preserve raw reports privately and publish reviewed path-redacted records without credentials or full private transcripts.

Publish success counts and uncertainty, infrastructure counts, per-task outcomes and end-to-end timing/resource use. Avoid small-sample superiority claims, extrapolation to unseen repositories, deterministic live-model claims or comparison with unmatched competitor accounts. This is a four-task retrospective pilot, not the stable-release baseline of twenty tasks per supported adapter.

All existing pilot artifacts remain byte-identical. Document new defects and create regression tests for demonstrated harness defects; generated candidate mistakes remain benchmark outcomes rather than automatically being declared runtime bugs.

## 5. Resource preflight

The host had approximately 731 MiB free during planning. Do not start downloads/builds/live trials until a documented disk/RAM/toolchain preflight passes. Inspect and reclaim only clearly owned, regenerable benchmark build caches where needed. Do not delete user data, original benchmark evidence, retained candidate branches, or the OS-consented native acceptance executable. Reuse verified read-only dependency caches and bound writable build products; dispose of owned build products after grading while retaining source/diffs and evidence. Record cache/setup timing separately from model task latency.

## 6. Ordered follow-on delivery

After the pilot and any demonstrated defects are reviewed, implement reserved budgets from the approved design: atomic applicable-scope allocations before every provider dispatch, idempotent settlement, durable uncertain allocations, unpriced/unbounded refusal and audited reconciliation. Then implement persistent schedules and finished-agent retention as separate reviewable increments, retaining the approved timezone, occurrence fencing, unattended permission and transcript-protection rules. Keep their original task checkboxes and user documentation synchronized with actual evidence. No live Auth or physical-device acceptance is added to this goal.
