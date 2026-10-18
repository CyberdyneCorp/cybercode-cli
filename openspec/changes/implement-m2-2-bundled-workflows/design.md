## Context

The M2.1 runtime runs Workflow modules: a pure-literal `export const meta` plus a default async function, executed by QuickJS on an engine thread per run (`crates/cyber-workflow/src/engine.rs`, `prelude.js`), with durable `agent()` results keyed by prompt, options and ordinal (`crates/cyber-server/src/runtime/workflow_host.rs`), budgets and `ctx.budget`, model tiers `fast`/`smart`/`verify`, write isolation into worktrees, and, since `add-parallel-workflow-runs`, a run queue, fair agent scheduling and the `/workflows` monitor.

Discovery (`discovery.rs`) scans `~/.config/cyber/workflows` and each project `.cyber/workflows` from the outermost ancestor to the Location; later roots replace earlier ones by `meta.name`. `module::Origin` is `Saved { scope, path }` or `Inline`. The `workflow` tool (`crates/cyber-tools/src/tools/workflow.rs`) starts a saved Workflow by name or an inline script and raises a `workflow.run` approval showing name, phases, budget, origin and digest.

`/review` and `cyber review` resolve a target with the `vcs-integration` resolver (sandboxed inspection helper, permission filtering, 256 KiB cap, `limitations`) and run the bundled P1 `review` skill in a read-only `reviewer` child whose structured result is the findings schema.

**Goals:** ship `review`, `audit`, `migrate`, `research` and `plan` inside the binary; make them discoverable and overridable; keep every one of them deterministic under replay, bounded in cost, and honest about what it did not cover; switch `/review` to the workflow.

**Non-Goals:** plugin-contributed Workflows (P4); remote runners (P3); automatic merging of migration branches; new tool types (the modules use existing tools only); a TUI args editor beyond a JSON form; changing the findings schema.

## Decisions

### 1. Embedding and origin

Each module is a plain `.js` file under `crates/cyber-workflow/src/bundled/` (`review.js`, `audit.js`, `migrate.js`, `research.js`, `plan.js`) plus `common.js` with shared helpers (finding validation, dedup keys, chunking, severity order). Modules cannot import, so the embedded source of each bundled module is `module.js` followed by `common.js`, concatenated at compile time with `concat!(include_str!(..), "\n", include_str!("bundled/common.js"))`. Function declarations in `common.js` are hoisted, and `meta` stays the first top-level declaration, so `meta::extract` reads it unchanged. The digest covers the concatenated source, so a new release with changed helpers is a new revision on resume, as for an edited saved module.

`Origin::Bundled { name }` is added. `bundled::modules()` parses all five once (a `OnceLock`), and a unit test fails the build if any does not parse or its `meta.name` differs from its file name. Discovery returns bundled entries first (lowest precedence), then global, then project; an entry replaced by a saved module lists `bundled:<name>` in `shadowed`. `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` and `workflows.bundled: false` hide them (for orgs that ship their own library).

Bundled runs record `origin: {kind: "bundled", name}` and the digest, and resume works as for saved modules: an upgrade that changes the source needs `resume --accept-revision` like an edited file.

### 2. Library surface

- `GET /api/v1/workflows` (`v1.workflow.library`) returns `{workflows: [{name, description, phases, args, budgets, origin, digest, shadowed}], issues: [...]}` for a `directory` query (the Location). The route replaces the `503` catch-all for that exact path; other unimplemented paths keep `503`.
- `cyber workflows list` adds an `ORIGIN` column (`bundled`, `global`, `project`) and `--format json`.
- `cyber workflows show <name>` prints meta, the args schema, default budgets, origin and, with `--source`, the module source.
- `cyber workflows eject <name> [--global] [--force]` writes the bundled source to `.cyber/workflows/<name>.js` (or the global root), refusing to overwrite without `--force`. The ejected copy then overrides the bundled one, so a project can tune prompts and lenses.
- TUI: the `/workflows` monitor gains a `Library` tab (`tab` switches between Runs and Library). It lists the library with origin badges; Enter opens a small form prefilled from `meta.args` defaults (JSON text, validated against the schema before posting) and starts the run for the current Session. `o` opens the source read-only.
- SDK: `client.workflows.library({ directory })`.

### 3. Direct read-only tools

`tools.read({path, offset?, limit?})`, `tools.glob({pattern, path?})` and `tools.grep({pattern, path?, include?, max?})` become callable from scripts when `meta.tools` (an allowlist array, e.g. `["glob", "grep"]`) or the invocation's `tools` option names them; otherwise `tools.<name>` rejects with `PermissionDenied: tool not allowed by meta.tools`. Mutating tools are never exposed.

Each call goes through the run owner's `ToolHost` with the same permission evaluation (the run's Mode as ceiling, `read` and `external_directory` rules, deny_read), output limits and sandbox as the built-in tool, but returns structured data (`{paths, truncated}`, `{matches: [{path, line, text}], truncated}`, `{content, truncated}`). A permission `ask` during a non-interactive run is rejected with `No interactive approver for workflow run`, as for agents.

Replay: a direct tool call is a host call. Its result is persisted as `workflow.host.settled.1` keyed by kind, normalized input and ordinal (the "Replay of workflow host calls" requirement), so a resumed run sees the same file list even if files changed. A changed input is a new call.

### 4. Shared conventions of the five modules

- **Args** are declared as a JSON Schema in `meta.args`; the host validates them before the run starts and rejects invalid args with `ExecutionFailure: args.<path> …` and no run record.
- **Budgets** are declared in `meta.budgets`, merged under invocation flags and `budgets.run` (the stricter value wins). `meta.estimate` is a literal string describing the agent count (e.g. "lenses × diff groups + one verifier per finding"); the `workflow.run` approval shows it next to the effective `max_agents` and `max_cost_usd`, which are the hard bounds.
- **Sizing.** Every fan-out is computed from args with a hard cap, logged when it drops work: `log({dropped: n, reason})`, and repeated in the result's `limitations` ("No silent caps").
- **Partial results.** Every `agent()` call is wrapped; a `BudgetExceeded` rejection stops further fan-out and the module returns what it has with `partial: true` and the budget named. Other agent failures become a `limitations` entry for that item, never a silent omission.
- **Tiers.** Finders, readers and drafters use `fast`; verifiers and judges use `verify`; synthesis uses `smart`. The tiers map through `workflows.tiers`, so a user with one model runs everything on it.
- **Agents.** Read-only phases use the `reviewer` agent (Plan ceiling, read/glob/grep and, for research, web tools); `migrate` transforms use `general` with `isolation: "worktree"` set explicitly.
- **Untrusted text.** File content, diff text and web pages reach agents only inside delimited data blocks with an instruction that they are data; findings are re-validated in code (the `vcs-integration` consumer rules) before they are kept.
- **Output.** Every module returns `{summary, ..., limitations, partial, stats: {agents, cost_usd, tokens}}` (JSON-safe, under the 1 MiB inline limit for normal inputs). The run's queued handback message to the owner Session carries the preview as today.
- **Determinism.** No `Date.now()`/`Math.random()`; ordering of items is by path then line so replay keys are stable.

### 5. `review`

Args: `target` (the `vcs-integration` grammar; default uncommitted changes), `lenses` (subset of `correctness`, `security`, `tests`, `maintainability`; default all four), `max_findings` (default 30, cap 60), `verify` (default `true`).

Preparation: when the run's module is the bundled `review` (or a saved module that sets `meta.prepare: "review_target"`), the host resolves `args.target` with the existing resolver under the reviewer's own permissions before the run starts and passes `args.diff = {base, head, files: [{path, status, patch}], limitations}`. Resolution errors are the existing `ReviewTargetError` codes, raised before any run record (HTTP 422, CLI exit 2). The script never runs git.

Phases:
1. `Group`: files sorted by path are packed into groups of at most 48 KiB of patch; a diff under 48 KiB is one group.
2. `Review`: one `reviewer` agent per (lens, group) with the findings schema; `pipeline()` so verification of a group starts as soon as its reviews return.
3. `Verify`: findings are validated and deduplicated by `(file, line, normalized summary)`; each remaining finding gets one `verify`-tier agent asked to refute it with evidence from the patch and the files (it may read files). Refuted findings are dropped and counted.
4. `Report`: findings ordered by severity, file, line, summary; capped at `max_findings` with the overflow counted in `limitations`.

Result: `{findings, limitations, lenses: {correctness: {raised, confirmed}, ...}, refuted, partial, stats}`, which conforms to the findings schema plus extra fields consumers ignore.

Budgets: `max_agents: 24`, `max_cost_usd: 2`, `max_wall_seconds: 1800`. Estimate: `lenses × groups + min(findings, max_findings)`.

`/review` and `cyber review`: when the bundled path wins (no `review` skill or command overrides it), they start the bundled `review` workflow through the same admission as today and show its findings exactly as the P1 view does (most severe first). The TUI follows the run instead of the reviewer Job. `cyber review` waits for the run and exits 0 with findings printed, 4 on `BudgetExceeded` (partial findings still printed), 1 on failure.

### 6. `audit`

Args: `scope` (glob, default `**/*`), `exclude` (globs; `.gitignore` already applies through `tools.glob`), `focus` (`bugs`, `security` or `both`; default `both`), `max_files` (default 400, cap 2000), `chunk_files` (default 12), `votes` (default 3 for critical/high).

Phases: `Scan` (`tools.glob`, then sort, cap and chunk by directory so related files stay together; files over 256 KiB are listed as skipped), `Find` (one `reviewer` per chunk with the findings schema and the focus checklist), `Dedup` (code), `Verify` (`votes` independent `verify` refuters for critical/high, majority survives; one for medium/low), `Report`.

Result: `{findings, coverage: {files_scanned, files_skipped: [{path, reason}], chunks}, refuted, limitations, partial, stats}`.

Budgets: `max_agents: 120`, `max_cost_usd: 10`, `max_wall_seconds: 7200`. Estimate: `chunks + findings × votes`.

### 7. `migrate`

Args (required: `instructions`, one of `files` or `pattern`): `instructions` (what to change), `files` (paths) or `pattern` (glob) with optional `contains` (a grep pattern that a file must match), `batch_size` (default 5, cap 25), `check` (shell commands run in each worktree after editing, e.g. `["cargo check", "cargo test -p foo"]`; default none), `confirm_over` (default 20 files), `max_batches` (default 40).

Phases:
1. `Select`: resolve the items with `tools.glob`/`tools.grep`, sorted; when the count exceeds `confirm_over`, `ask("Migrate N files in B batches?")` unless `args.yes` is true. In a non-interactive run without `yes` this rejects with `NoInteractiveUser`, before any agent starts.
2. `Transform`: one `general` agent per batch with `isolation: "worktree"` and the instructions, the batch's paths and the check commands. The agent edits only its batch's files, runs the checks with `bash` in its worktree, commits on its branch, and returns `{status: "done"|"failed", changed: [paths], check: [{command, exit_code, tail}], notes}`. Runs under the owner's Mode and rules as the ceiling; `bash` and `edit` asks in a non-interactive run are auto-rejected as today, so CI use needs allow rules (documented in the module description).
3. `Verify`: a `verify` agent per batch reads the branch diff (via the worktree's files) and the check output and returns `{ok, problems}`; a batch with failing checks is not marked ok.
4. `Report`.

Result: `{batches: [{index, files, branch, status, checks, verified, problems}], merged: false, integrate: "git merge <branches…>" suggestion, limitations, partial, stats}`. Nothing is merged; worktrees stay until the user removes them (`cyber worktrees`), so the user can inspect each branch.

Budgets: `max_agents: 2 × max_batches`, default cap 80; `max_cost_usd: 15`; `max_wall_seconds: 14400`. Estimate: `2 × batches`.

### 8. `research`

Args: `question` (required), `sources` (subset of `web`, `repo`, `docs`; default `["repo", "docs"]`, plus `web` when the `websearch` and `webfetch` tools are permitted for the reviewer agent in this Location), `angles` (default 3, cap 6), `max_sources` (default 12).

Phases: `Sweep` (one searcher per (source, angle), each told a different angle and modality, returning candidate sources with why), `Read` (dedup sources by URL/path; one reader per source extracts claims with quotes and locations), `Cross-check` (each claim that matters to the answer is checked by an independent `verify` agent against the other sources; a claim with one source is `verified: false`), `Synthesize` (`smart` agent writes the answer from verified claims, citing them; unverified claims appear only in a separate section).

Result: `{answer, claims: [{text, sources: [ref], verified, confidence}], sources: [{ref, kind, title}], unverified, limitations, partial, stats}`. When `web` was requested but not permitted, `limitations` says so and the run continues with the other sources.

Budgets: `max_agents: 40`, `max_cost_usd: 5`, `max_wall_seconds: 3600`. Estimate: `sources × angles + max_sources + claims`.

### 9. `plan`

Named `plan` as in the workflows spec (the roadmap calls it plan-from-angles). Args: `task` (required), `angles` (default `["mvp-first", "risk-first", "user-first"]`, 2–6), `judges` (default 3), `context` (optional paths to read first).

Phases: `Draft` (one read-only `reviewer` planner per angle on `smart`, each returning `{title, steps: [{step, files, risk}], assumptions, risks, tests}`), `Judge` (each judge scores every draft on a fixed rubric: correctness, completeness, risk, effort, testability, 1–5 each, with reasons), `Merge` (`smart` agent takes the highest-scoring draft and grafts named improvements from the others, listing each graft's source).

Result: `{plan_markdown, chosen: angle, scores: {angle: {total, by_criterion}}, grafts: [{from, what}], drafts, limitations, partial, stats}`. The plan is text only; nothing is edited.

Budgets: `max_agents: 14`, `max_cost_usd: 3`, `max_wall_seconds: 1800`. Estimate: `angles + judges + 1`.

### 10. Model guidance

The `workflow` tool description lists the library names visible in the Location with one line each (bounded to 20 entries, bundled ones first) and says to prefer a library Workflow over an inline script when one fits. `workflows.auto` guidance (when not `off`) names `review`, `audit`, `migrate`, `research` and `plan` with their args. The approval shows the origin `bundled:<name>`.

## Testing approach

All tests run without network or a real provider:

- `crates/cyber-workflow` unit tests: every bundled module parses, names match files, args schemas validate their defaults, helper functions (`common.js`) through a small QuickJS harness.
- Engine tests with scripted agents (`crates/cyber-server/tests/workflow_runs.rs` style): each module's phases, caps, dedup, partial results on `BudgetExceeded`, refuted findings, replay after a restart with identical `tools.*` results.
- Production path: `crates/cyber-app/tests/bundled_workflows.rs` with the OpenAI-compatible scripted provider (`tests/support/scripted_model.rs`), responding per prompt label so each agent gets a deterministic structured result; real HTTP, real worktrees in a temporary git repository.
- CLI: `crates/cyber-cli/tests/workflows_cli.rs` with the real binary: `list` origin column, `show`, `eject`, `run <bundled> --wait` exit codes, `cyber review` on a scripted provider.
- TUI: `TestBackend` + insta for the Library tab and `/review` findings from a run.
- Acceptance: the real `./target/debug/cyber` with `scripts/screens/provider.py`-style scripted provider, captured from a pty for the TUI surfaces.

## Risks / Trade-offs

- **Cost surprise on `/review`.** Mitigated by small default budgets, one group for small diffs, the approval showing the estimate, and the override path.
- **Bundled prompt quality.** Prompts are product surface; they live in plain `.js` files, are covered by golden tests of the prompts' fixed parts, and users can eject and edit.
- **Concatenated helpers change the digest.** Any helper edit is a new revision for every bundled module; acceptable, since a release is a revision anyway.
- **migrate in CI.** Non-interactive runs auto-reject asks; without allow rules for `edit` and `bash` every batch fails. The module checks permissions up front through a dry run (`permissions test` path) and fails early with the rules it needs, instead of spawning agents that will be refused.
