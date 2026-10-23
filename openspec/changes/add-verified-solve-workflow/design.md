## Context

The server already runs QuickJS workflows with real child Sessions, fair scheduling, soft budgets, replay and managed worktrees. M2.2 proposes discovery, origin badges, schemas and a library surface. `solve` composes those facilities, but adds host-owned verification so model output cannot manufacture passing test evidence.

## Decisions

### 1. Public invocation and defaults

Use `cyber workflows run solve --args '{"task":"fix the failing parser test","checks":[{"argv":["cargo","test","--locked"]}]}' --wait` or the equivalent workflow API/tool/library form.

Args:

- `task`: nonempty string, at most 32 KiB UTF-8.
- `candidates`: two to four `{ label, model }` entries; unique labels (1–48 characters, letters/digits/underscore/hyphen), `model` a configured ref or tier. Defaults: `fast`, `smart`, `verify`, with those labels.
- `checks`: one to eight `{ argv, timeout_seconds? }` entries; nonempty argv, at most 256 arguments, 32768 UTF-8 bytes per argument and 131072 bytes per check, no NUL or implicit shell expansion; default timeout 300 seconds, range 1–3600. `sh -c` or another explicit shell remains an ordinary permission-checked command.
- `judge_model`: model ref or tier, default `verify`.

Resolve and record each model ref before launching candidates. Missing models, no Git repository, tracked modifications or non-ignored untracked files fail before creating child Sessions. Never stash user changes. Record the clean HEAD commit and checkout identity as the common base. Ignored build dependencies are recreated by existing sandboxed worktree setup; disclose missing prerequisites rather than copying credentials or claiming equivalent environments.

Defaults: `max_agents: 5`, `max_cost_usd: 5`, `max_wall_seconds: 1800`; candidate agents have at most 20 steps each. Use `meta.budget` and the existing Run budgets resolution: module/model inputs only lower inherited ceilings, while explicit authenticated API/CLI overrides retain their existing semantics. Owner Session budgets and permission ceilings still apply. These are soft dispatch limits: in-flight work can overshoot, and unpriced/unknown usage makes cost incomplete. The approval shows resolved models, candidate count, checks and effective budgets before launch.

### 2. Solve independently

Every candidate gets a fresh child Session and managed worktree from the same base, the same task and check definitions, and the owner's effective permission ceiling. It does not receive other candidates' transcripts. Each branch is retained through the normal retention/cleanup policy and listed in the result; no candidate may edit the owner or sibling worktrees.

After a candidate settles, record its final source snapshot and diff digest. Settlement must cover its owned tools and Jobs. A cancelled/unknown native operation or worktree setup is not a completed candidate and prevents that candidate from advancing to verification.

### 3. Authoritative checks

Add a host-owned verification phase, callable only by the bundled protocol under the captured run/candidate authority. It runs the user's recorded argv commands in that candidate's checkout through the existing sandbox, permission/hook, environment, cancellation and native-settlement paths. This is not a generic mutating `tools.*` API for QuickJS.

Fence the candidate against further agent writes while checks run. Fingerprint the submitted source before and after each check; a source change invalidates the check. Build outputs outside the submitted source are permitted under ordinary sandbox rules. Record argv, working directory, start/end, timeout/cancellation, exit status, source digest and stdout/stderr references. Store at most 1 MiB of combined log per check, mark truncation, and keep the full declared check list including checks not run. Test scripts are untrusted execution; they run with the same ceilings as candidate tools.

API/client verification badges derive only from matching host records, never a module name or returned JSON. A project override named `solve` can return arbitrary output, but cannot manufacture host evidence.

The report distinguishes `passed`, `failed`, `blocked`, `timed_out`, `cancelled`, `unknown` and `not_run`. Only exit 0 with verified settlement and unchanged source is `passed`. An agent's own test runs can be useful context but never substitute for host evidence. Unknown outcomes stay fenced until reconciled; never rerun them automatically just because the client disconnected.

### 4. Independent judge and recommendation

Only candidates for which every required host check passed are eligible. A new read-only reviewer Session, not any solver Session, receives the task, base, eligible diffs, host evidence and explicit limitations. Candidate material and logs are delimited untrusted data. The judge cannot write, run candidate code or promote a branch.

Use a documented rubric: correctness 40%, regression/security risk 25%, maintainability 20%, scope adherence 15%. Scores explain a recommendation; they never override eligibility. The judge may reject every candidate, and a missing/invalid judge result yields no recommendation. A different judge model is encouraged but not mandatory; record the actual resolved model and whether it differs from the solvers. Do not label same-model attempts as model diversity.

### 5. Durable result and resume

Return the standard library envelope plus `base_commit`, `candidates[]`, `judge`, `recommended_candidate` (nullable) and `verification_status`. Each candidate carries its child Session, branch/worktree, resolved model, snapshot/diff digests, state, checks, usage and limitations. `verification_status` is `recommended`, `no_candidate_passed`, `judge_unavailable` or `incomplete`; it describes the report, not a replacement Workflow Run lifecycle enum. A complete run may produce no recommendation; CLI consumers inspect the report alongside the existing workflow exit code.

Verification records are durable, keyed by run, candidate, source digest, check-definition digest and ordinal. Resume reuses only settled records with matching identities. Changed source/checks require a revision and new evidence; unresolved work remains explicit. Once records exist, losing the client never launches duplicate solver Sessions or checks. Preserve partial results on budget, cancellation or failure. Existing run events, artifacts, usage and retention carry the report; any new event/schema projection needs a migration and replay test before implementation is accepted.

### 6. Library delivery order

Implement the shared M2.2 foundation, then `solve` end to end through CLI and HTTP/SDK, then TUI display. The five originally planned modules already ship (`implement-m2-2-bundled-workflows`), so the foundation is reused, not rebuilt. Listings expose only modules that actually ship, with their real origin; no placeholder entry is offered as runnable. The dashboard consumes the public report after it exists.

## Verification

Use scripted providers with intentionally good, bad and deceptive candidates, real temporary Git worktrees, authenticated App HTTP requests and the real CLI. Test same-model and mixed-model resolution, failed/blocked/timed-out/unknown checks, source mutation, an agent falsely claiming success, a judge selecting an ineligible candidate, restart at every phase, budget interruption and unchanged owner files. A separate opt-in live multi-provider run establishes practical behavior without asserting deterministic output or competitor parity.

## Tradeoffs

Several attempts cost more than one. Verification can still miss bugs. A clean Git base initially excludes dirty-checkout tasks. Soft budgets cannot promise a hard spend ceiling. These limits are visible in the approval and result; the workflow provides inspectable evidence and a recommendation for human review.

## Implementation (2026-10-23)

The start path recognizes `meta.prepare: "solve"`. A pure typed parser applies defaults and rejects invalid/unknown fields, including client-supplied `solve_admission`. After resolving all refs against Location tiers and the owner model, the host reads HEAD/root metadata through the existing sandboxed Git execution port with optional index writes disabled, and uses the existing read-only libgit2 helper for dirty-file inspection so repository clean filters cannot execute. It rejects dirty/unborn/non-Git checkouts. It reads HEAD before and after status and refuses a changed base. Explicit owner read/external-directory denials apply before reading the corresponding root.

Normalized args retain a `solve_admission` object containing the commit, repository paths, requested/effective candidate models, judge ref and count of distinct solver models (variants of one provider/model do not create model diversity). These are admission observations, not execution leases or verified-check records. Saved/inline modules may request read-only preparation, but gain no mutating tool or verified-result authority from it. The embedded solve entry alone can invoke the dedicated verification bridge; both origin and source digest are checked. Its internal worktree recipe pins the recorded commit. General workflows retain configured-base behavior.


The libgit2 inspection request optionally carries a pinned worktree base. Its solve-only source fingerprint hashes sorted tracked and non-ignored paths, raw file contents and modes, deletions and symlink targets without executing Git filters. Unsupported source types and inspection limits fail closed. Review-only requests retain their previous behavior.

Candidate submission records `workflow.solve.submitted.1`; ancestry admission checks reject later solver/descendant work. The host callback captures a narrowly scoped verification capability for its own native activity and hooks, including transaction-time guard checks; ordinary clients cannot grant it. Pending subtree actors and Jobs prevent verification. Native checkout leases stay unknown when evidence cannot establish settlement. Command sandboxing narrows full-access/extra writable roots and ambient temporary-directory access for solve children, and file tools refuse writes outside the candidate. The judge is forced into Plan mode with only read/glob/grep tools, independent of profile overrides.

`workflow.solve.evidence.1` records snapshots and check intent/settlement under deterministic run/revision/candidate/ordinal aggregates. Existing event storage needs no new projection table. A pending intent is returned as unknown and never redispatched. Checks use literal argv with permissions and pre/post hooks, owned process-tree settlement and bounded output. Submitted-source mutation invalidates a passing exit. Unknown outcomes prevent subsequent native checks. Log ordering is labelled stdout-then-stderr.

The detail API derives `solve_evidence` directly from matching ledger identities, not module JSON. It remains available for partial runs; the existing result-file mechanism carries large reports. SDK types include the authority field and the terminal monitor renders its outcomes, source digest and branch. Usage reports disclose incomplete pricing, effective soft limits and measured overshoot. No paid live-model acceptance or quality-parity claim is included in scripted acceptance.
