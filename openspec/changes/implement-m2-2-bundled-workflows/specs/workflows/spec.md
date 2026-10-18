## MODIFIED Requirements

### Requirement: Saved and bundled workflows
(P2) The system SHALL discover saved Workflows from `.cyber/workflows/**/*.{ts,js}` (project), `~/.config/cyber/workflows/` (global) and plugins, and SHALL bundle `review` (multi-angle code review with verification), `audit` (codebase-wide bug or security sweep), `migrate` (file-by-file migration with build and test checks), `research` (cross-checked research with sources) and `plan` (plans drafted from several independent angles, then merged). Project workflows SHALL override global and bundled ones with the same name.

Bundled Workflows SHALL be embedded in the `cyber` binary, carry the origin `bundled` with their name, and have the lowest precedence: global roots override them and project roots override both. A listing SHALL mark every entry's origin (`bundled`, `global` or `project`) and name the definitions it shadows. `workflows.bundled: false` or `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` SHALL hide the bundled Workflows from listings, the `workflow` tool and runs by name. `cyber workflows show <name>` SHALL print a library entry's meta, args schema, default budgets and origin (and with `--source` its source), and `cyber workflows eject <name> [--global] [--force]` SHALL copy a bundled Workflow's source to `.cyber/workflows/<name>.js` (or the global root), refusing to overwrite an existing file without `--force`. A run of a bundled Workflow SHALL record its origin and source digest, and a release that changes a bundled source SHALL be treated on resume like an edited saved module.

#### Scenario: Run a bundled workflow
- **WHEN** the user runs `cyber workflows run review --args '{"target":"main..HEAD"}'`
- **THEN** the bundled review workflow starts against the diff between the resolved commits of `main` and `HEAD`

#### Scenario: Bundled listed on a fresh install
- **WHEN** a user with no saved Workflows runs `cyber workflows list`
- **THEN** `review`, `audit`, `migrate`, `research` and `plan` are listed with origin `bundled`

#### Scenario: Ejected copy overrides the bundled one
- **WHEN** the user runs `cyber workflows eject audit`, edits `.cyber/workflows/audit.js`, and runs `cyber workflows run audit`
- **THEN** the edited project module runs, and `cyber workflows list` shows `audit` with origin `project` shadowing `bundled:audit`

#### Scenario: Eject refuses to overwrite
- **WHEN** `.cyber/workflows/review.js` exists and the user runs `cyber workflows eject review`
- **THEN** the command exits 1 naming the file and leaves it unchanged

## ADDED Requirements

### Requirement: Bundled workflow contracts
(P2) Each bundled Workflow SHALL declare its args as a JSON Schema in `meta.args`, its default budgets in `meta.budgets` and an agent-count description in `meta.estimate`, and the host SHALL reject invalid args before creating a run with `ExecutionFailure` naming the args path. Effective budgets SHALL be the stricter of the module's defaults, invocation options and `budgets.run`, and the `workflow.run` approval SHALL show them with the estimate. Every bundled Workflow SHALL cap each fan-out, record dropped work in its result's `limitations`, return its partial result with `partial: true` when a budget stops new agents, record an agent failure for an item in `limitations` rather than omit it, pass file, diff and web content to agents only as delimited data, and return `{summary, limitations, partial, stats}` plus its own fields:

- `review`: args `target` (the `vcs-integration` review target grammar, default uncommitted changes), `lenses` (subset of `correctness`, `security`, `tests`, `maintainability`, default all), `max_findings` (default 30, at most 60) and `verify` (default true). Before the run starts, the host SHALL resolve `target` with the review target resolver under the reviewer's permissions and pass the filtered diff as `args.diff`; resolution errors SHALL be reported as `ReviewTargetError` with no run created. Read-only reviewers per lens and file group SHALL raise findings in the `vcs-integration` findings schema; findings SHALL be re-validated, deduplicated and, when `verify` is true, each checked by an independent `verify`-tier agent that may refute it. The result SHALL contain `findings` ordered by severity, file, line and summary, `refuted` and per-lens counts. Default budgets: 24 agents, 2 USD, 30 minutes.
- `audit`: args `scope` (glob, default all files), `exclude`, `focus` (`bugs`, `security` or `both`), `max_files` (default 400, at most 2000), `chunk_files` (default 12) and `votes` (default 3). Read-only finders per chunk SHALL raise findings in the findings schema; critical and high findings SHALL survive only if a majority of `votes` independent refuters fail to refute them; medium and low findings SHALL get one verifier. The result SHALL contain `findings`, `refuted` and `coverage` with the files scanned and every skipped file with its reason. Default budgets: 120 agents, 10 USD, 2 hours.
- `migrate`: args `instructions` (required), `files` or `pattern` (one required) with optional `contains`, `batch_size` (default 5, at most 25), `check` (commands), `confirm_over` (default 20) and `yes`. When more than `confirm_over` files are selected and `yes` is not true, the run SHALL `ask()` for confirmation before any agent starts. Each batch SHALL run in its own worktree with an edit-capable agent that runs the `check` commands there and commits on its branch, followed by a verifier of the branch's diff and check output. The result SHALL list every batch with files, branch, status, check exit codes and verification, and SHALL NOT merge any branch. Before starting agents, the run SHALL check that the owner's rules allow the edits and commands it needs in a non-interactive run and fail with the missing rules otherwise. Default budgets: 80 agents, 15 USD, 4 hours.
- `research`: args `question` (required), `sources` (subset of `web`, `repo`, `docs`), `angles` (default 3, at most 6) and `max_sources` (default 12). Searchers SHALL run per source and angle, readers SHALL extract claims with locations, and each claim used in the answer SHALL be cross-checked by an independent agent; a claim supported by one source SHALL be marked `verified: false`. When `web` is requested but the web tools are not permitted, the result SHALL say so in `limitations` and use the other sources. The result SHALL contain `answer`, `claims` with sources and `verified`, and `sources`. Default budgets: 40 agents, 5 USD, 1 hour.
- `plan`: args `task` (required), `angles` (2–6, default `mvp-first`, `risk-first`, `user-first`), `judges` (default 3) and `context` (paths). Read-only planners SHALL draft one plan per angle, judges SHALL score every draft on correctness, completeness, risk, effort and testability, and a final agent SHALL merge the highest-scoring draft with named grafts from the others. The result SHALL contain `plan_markdown`, `chosen`, `scores` and `grafts`, and no file SHALL be edited. Default budgets: 14 agents, 3 USD, 30 minutes.

`/review` and `cyber review` SHALL run the bundled `review` Workflow when no discovered `review` skill or command overrides it, and SHALL report its findings as the `vcs-integration` "Local review command" requires; `cyber review` SHALL exit 0 when the run completes, 4 when a budget stopped it (printing the partial findings) and 1 when it fails.

#### Scenario: Review verifies findings
- **WHEN** `/review` runs on a diff where the correctness lens raises two findings and the verifier refutes one
- **THEN** the TUI shows one finding, and the run result has `refuted: 1`

#### Scenario: Invalid review target creates no run
- **WHEN** the user runs `cyber workflows run review --args '{"target":"main.."}'`
- **THEN** the command fails with `ReviewTargetError` code `invalid_target` and `GET /api/v1/workflows/runs` lists no new run

#### Scenario: Audit reports skipped files
- **WHEN** `audit` runs with `max_files: 100` over a scope of 140 files
- **THEN** the result's `coverage` lists 100 files scanned and 40 skipped with reason `max_files`, and `limitations` repeats the cap

#### Scenario: Majority refutes an audit finding
- **WHEN** an audit finder raises a high finding and two of three refuters refute it
- **THEN** the finding is absent from `findings` and counted in `refuted`

#### Scenario: Migrate asks before a large run
- **WHEN** `migrate` selects 60 files with the default `confirm_over` in an interactive Session
- **THEN** the run waits in `waiting_input` with the question naming 60 files and 12 batches, and no agent has started

#### Scenario: Migrate batches stay on branches
- **WHEN** `migrate` completes 3 batches with `check: ["cargo check"]`
- **THEN** the result lists 3 branches with the check exit codes, and the owner's checkout is unchanged

#### Scenario: Migrate in CI without allow rules
- **WHEN** `cyber workflows run migrate --wait` runs non-interactively and no rule allows `edit`
- **THEN** the run fails before starting any agent, naming the missing `edit` rule

#### Scenario: Research without web access
- **WHEN** `research` is started with `sources: ["web", "repo"]` and the web tools are denied
- **THEN** the answer is built from repository sources and `limitations` says web sources were not permitted

#### Scenario: Plan merges the winner
- **WHEN** `plan` drafts three plans and the risk-first draft scores highest
- **THEN** the result's `chosen` is `risk-first` and every graft names the draft it came from

#### Scenario: Budget stops a fan-out
- **WHEN** `audit` runs with `--max-cost 1` and the budget is reached after 6 of 20 chunks
- **THEN** the run ends `budget_exceeded`, its result has `partial: true` with the findings of the finished chunks, and `cyber workflows run --wait` exits 4

### Requirement: Workflow library over HTTP
(P2) `GET /api/v1/workflows` SHALL return the Workflow library of a Location (`directory` query): each saved and bundled entry with `name`, `description`, `phases`, `args`, `budgets`, `estimate`, `origin`, `digest` and `shadowed`, and the modules that failed to load with their diagnostics, applying the same precedence and the same `workflows.bundled` setting as the CLI. The `workflow` tool description SHALL name the library entries visible in the Location (at most 20, bundled first) and SHALL tell the model to prefer a library Workflow over an inline script when one fits.

#### Scenario: Library over HTTP
- **WHEN** a client calls `GET /api/v1/workflows?directory=/repo` in a project with `.cyber/workflows/release.js`
- **THEN** the response lists `release` with origin `project` and the five bundled Workflows with origin `bundled`

#### Scenario: Model starts a bundled workflow by name
- **WHEN** the user asks the model to audit the auth module for security bugs
- **THEN** the model calls the `workflow` tool with `name: "audit"` and args `{scope: "src/auth/**", focus: "security"}`, and the approval shows origin `bundled:audit` with its budgets and estimate
