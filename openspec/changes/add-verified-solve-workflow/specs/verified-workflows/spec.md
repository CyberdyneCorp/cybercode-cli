## Purpose
Verified workflows produce reviewable candidate patches with host-recorded check evidence and an independent recommendation. Passing declared checks is evidence, not proof of correctness.

## ADDED Requirements

### Requirement: Solve workflow arguments and admission
(P2) The bundled `solve` Workflow SHALL accept `task` (nonempty, at most 32768 UTF-8 bytes), `candidates` (2–4 uniquely labelled model refs or tiers, default `fast`, `smart`, `verify`), `checks` (1–8 `{ argv, timeout_seconds? }` objects with nonempty argv, timeout default 300, range 1–3600) and `judge_model` (default `verify`). It SHALL resolve and record models and a clean Git HEAD before starting child Sessions; a non-Git checkout, tracked changes, non-ignored untracked files, invalid args or an unavailable model SHALL fail before children are created, without stashing or modifying user files. Default `meta.budget` SHALL set 5 agents, 5 USD and 1800 seconds, with at most 20 steps per solver. Effective budgets SHALL resolve under `workflows` → Run budgets, including explicit authenticated overrides; module/model inputs SHALL NOT widen inherited ceilings. All owner permission and Session budget ceilings SHALL apply. The approval SHALL show models, candidate count, checks and effective soft budgets, including possible in-flight overshoot and incomplete pricing.

#### Scenario: Mixed-model attempt
- **WHEN** the user starts `cyber workflows run solve --args` with three valid model refs and a check argv in a clean Git checkout
- **THEN** the approved run records that base and those resolved models before any candidate starts

#### Scenario: Dirty checkout
- **WHEN** the owner has uncommitted tracked changes or a non-ignored untracked file
- **THEN** admission fails with an actionable clean-checkout error, starts no child and leaves every file unchanged

#### Scenario: One configured model
- **WHEN** the three default tiers all resolve to one model
- **THEN** the workflow may run three attempts and reports one distinct model rather than claiming model diversity

### Requirement: Independent isolated candidate execution
(P2) Each solver SHALL use its own child Session and managed worktree from the recorded common base, under the owner's effective tool, sandbox, Mode and budget ceilings. Solvers SHALL not receive sibling transcripts or write sibling or owner checkouts. A candidate SHALL advance only after its owned tools and Jobs have settled; its source snapshot/diff digest, Session, branch and usage SHALL be retained in the run report. Setup failures and unknown native ownership SHALL remain explicit and SHALL NOT become successful candidates.

#### Scenario: Competing edits
- **WHEN** two solvers edit the same source path
- **THEN** their changes exist on separate candidate branches and the owner's file is unchanged

#### Scenario: Unsettled background work
- **WHEN** a solver finishes its answer while an owned Job still has unknown native settlement
- **THEN** that candidate is fenced from verification and is reported as unresolved

### Requirement: Host-owned candidate verification
(P2) Required checks SHALL be executed by the host in the captured candidate checkout through ordinary permission, hook, sandbox, environment, timeout and native-settlement boundaries, using the recorded argv with no implicit shell. Model claims SHALL NOT establish a passing check. The host SHALL fence further solver writes and bind evidence to run, candidate, source digest, check-definition digest and ordinal; submitted source SHALL include tracked contents and non-ignored candidate additions, represented by a snapshot with stable path/content digests. A changed submitted source during a check SHALL invalidate its evidence. Each check SHALL record argv, directory, timing, exit/settlement outcome and at most 1 MiB of combined output with truncation marked. Outcomes SHALL distinguish `passed`, `failed`, `blocked`, `timed_out`, `cancelled`, `unknown` and `not_run`; only exit 0 with verified settlement and unchanged source SHALL pass. All declared checks, including those not executed, SHALL appear in the report.

#### Scenario: Agent fabricates success
- **WHEN** a solver says all tests passed but the host's recorded test process exits 1
- **THEN** the check is `failed` and the candidate is ineligible

#### Scenario: Verification changes source
- **WHEN** a check rewrites a submitted source file before exiting 0
- **THEN** its evidence is invalidated and it does not qualify the candidate

#### Scenario: Denied check
- **WHEN** the owner's effective rules deny a required check command
- **THEN** no process is launched, the check is `blocked`, and the workflow does not mark the candidate verified

### Requirement: Independent judge and candidate eligibility
(P2) Only candidates for which every required host check passed SHALL be eligible for recommendation. A fresh read-only reviewer Session, separate from all solvers, SHALL judge the task, eligible diffs and host evidence with a rubric weighted correctness 40%, regression/security risk 25%, maintainability 20% and scope adherence 15%. Candidate text and logs SHALL be untrusted data. The host SHALL validate the judge's structured result, refuse selection of an ineligible candidate and allow rejection of all candidates. A missing, invalid or unavailable judge SHALL yield no recommendation. The result SHALL disclose the resolved judge model and whether it differs from solver models; it SHALL NOT automatically merge, apply to the owner checkout or publish a PR.

#### Scenario: Judge selects failing patch
- **WHEN** a judge selects a candidate whose required check failed
- **THEN** the host rejects that recommendation and leaves the candidate branches and owner checkout unmerged

#### Scenario: Tests pass but patch is unsafe
- **WHEN** all required checks pass but the judge finds every patch unacceptable
- **THEN** `recommended_candidate` is null with the judge's reasons and the passing checks still visible

### Requirement: Verified result evidence and recovery
(P2) `solve` SHALL return the library envelope `{ summary, limitations, partial, stats }` plus the base commit, every candidate's identifiers, resolved model, source/diff digest, checks, usage and limitations, judge evidence and nullable `recommended_candidate`. `verification_status` SHALL be `recommended`, `no_candidate_passed`, `judge_unavailable` or `incomplete`; it SHALL NOT replace the Workflow Run lifecycle or existing CLI exit mapping. Check records SHALL be durable before becoming visible as settled. Only matching server-retained evidence SHALL authorize a host-verified label; a saved/overriding module named `solve` or a returned JSON field SHALL NOT manufacture that authority. Resume SHALL reuse only matching settled identities; source or check changes SHALL require a revision and new evidence. Unknown outcomes SHALL remain fenced without automatic redispatch. Budget stops, interruptions and failures SHALL retain partial evidence and incomplete/unknown cost, and client disconnect SHALL NOT duplicate solvers or checks.

#### Scenario: Restart after checks settle
- **WHEN** the server restarts after checks are recorded and before judging
- **THEN** resume reuses those checks only for their exact recorded source/check identities and starts no duplicate solver

#### Scenario: Budget stops an attempt
- **WHEN** the soft budget blocks new dispatch after two candidates settle
- **THEN** the report preserves their evidence, lists the remaining work as incomplete and discloses actual recorded spend and pricing limits

#### Scenario: Overriding module fabricates evidence
- **WHEN** a project Workflow named `solve` returns invented passing-check JSON with no matching host records
- **THEN** the API and clients treat it as ordinary unverified module output and do not display a host-verified recommendation
