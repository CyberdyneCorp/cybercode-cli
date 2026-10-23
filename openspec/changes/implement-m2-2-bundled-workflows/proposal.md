## Delivery update (2026-10-22)

The five original modules shipped first. The verified [`solve`](../archive/2026-10-23-add-verified-solve-workflow/proposal.md) extension shipped in PR #23: independent model attempts, authoritative checks and an independent recommendation. It reuses sections 1–4 of this change; its reviewed contracts, design and acceptance evidence are archived in that linked change. "Five modules" below refers to this change's original scope; the shipped library now has six.

## Why

ROADMAP M2.2 ("Bundled workflows: review, audit (find → verify), migrate (fan-out with worktrees), research (cross-checked), plan-from-angles") is not implemented. M2.1 (`fc61f9f`) and `add-parallel-workflow-runs` (`0b784c0`) ship the runtime: the QuickJS engine, `agent()`/`parallel()`/`pipeline()`/`phase()`, durable resume, budgets, model tiers, write isolation, the run queue and the `/workflows` monitor. What users can run, though, is only what they write themselves:

- `workflows` → "Saved and bundled workflows" requires five bundled modules. `crates/cyber-workflow/src/discovery.rs` discovers only global and project roots, and `module::Origin` has no bundled variant. `cyber workflows list` shows nothing on a fresh install.
- `vcs-integration` → "Local review command" says that from P2 `/review` and `cyber review` run the bundled `review` workflow, which checks several angles in parallel and verifies each finding. Today they still run the single-agent P1 `review` skill.
- The M2.1 gap work now ships direct read-only `tools.read/glob/grep` and saved-Workflow listing over HTTP (`GET /api/v1/workflows`). Reuse that foundation for bundled discovery, args schemas and the client Library surfaces; those shipped pieces are not new work in this change.

Without bundled workflows, the orchestration features are only reachable by people who write workflow scripts. The roadmap promises them out of the box, matching Claude Code's bundled `/review` multi-agent review and its dynamic workflows.

## What Changes

- **Bundled origin.** A new `Origin::Bundled { name }`. Five modules are embedded in the `cyber` binary (`crates/cyber-workflow/src/bundled/*.js`, `include_str!`). Discovery lists them below global and project roots, so a saved Workflow with the same `meta.name` overrides them. Listings mark each entry `bundled`, `global` or `project` and name what it shadows.
- **Library surface.** `GET /api/v1/workflows` lists the library (saved and bundled, with issues). `cyber workflows list` shows the origin column. New `cyber workflows show <name>` prints a library entry's meta, args schema and source, and `cyber workflows eject <name> [--global]` copies a bundled module into `.cyber/workflows/<name>.js` for customization. The `/workflows` TUI monitor gets a Library tab that starts a run with an args form.
- **Direct read-only tools (M2.1 remainder).** `tools.read`, `tools.glob` and `tools.grep` callable from scripts when `meta.tools` or the invocation allowlists them, with the built-in tools' permission checks and limits, recorded for replay.
- **`review`.** Input: a host-resolved, permission-filtered diff (the `vcs-integration` resolver, also used when started by name with `args.target`). Four lenses (correctness, security, tests, maintainability) on the `fast` tier over file groups, then one `verify`-tier verification per finding that must refute or confirm. Output: the existing findings schema plus `limitations` and per-lens counts. `/review` and `cyber review` switch to it; an overriding `review` skill or command still wins.
- **`audit`.** A codebase-wide bug and security sweep: scope by `tools.glob`, partition into bounded chunks, read-only finders per chunk, dedup in code, adversarial verification (3 votes for critical/high, 1 otherwise), a completeness note listing skipped files. Read-only end to end.
- **`migrate`.** File-by-file migration: `tools.glob`/`grep` select the items, `ask()` confirms above a threshold, one edit-capable agent per batch in its own worktree runs the given `check` commands there, a verifier reviews each batch's diff and check output. Output: per-batch branch, status and check log; nothing is merged automatically.
- **`research`.** Cross-checked research: parallel searchers by modality (web when the web tools are permitted, the repository, local docs), deep reads, claim extraction, independent cross-checks, and a synthesis whose claims each carry sources and a `verified` flag.
- **`plan`** (the roadmap's plan-from-angles). Independent read-only planners from different angles (default MVP-first, risk-first, user-first), parallel judges with a rubric, then a merge of the winner with grafts from runners-up.
- **Cost guards.** Every bundled module declares `budgets` in `meta` (`max_agents`, `max_cost_usd`, `max_wall_seconds`) and sizes its fan-out from args with hard caps; the `workflow.run` approval shows them with an agent-count estimate; `--max-cost` and `budgets.run` still cap them; when a budget stops new agents the script returns a partial result marked `partial: true`.
- **Model guidance.** The `workflow` tool description and `workflows.auto` guidance name the bundled workflows and their args, so the model can start `review`, `audit`, `migrate`, `research` or `plan` by name instead of drafting an inline script.

## Capabilities

### Modified Capabilities
- `workflows`: MODIFIED "Saved and bundled workflows" (bundled origin, precedence, listing, eject, per-module args and outputs). ADDED "Bundled workflow contracts" (P2: args, phases, outputs, permissions and default budgets of the five modules) and "Workflow library over HTTP" (P2).
- `vcs-integration`: no delta. "Local review command" already says P2 runs the bundled `review` workflow with the same findings schema; this change implements it.
- `tui`: no delta. The Library tab is part of the existing "Workflow monitor" requirement's "open the script" and run start; the design records the layout.
- `cli-commands`: no delta. `cyber workflows` is registered; `show` and `eject` are subcommands of it, specified in the workflows delta.

## Impact

- Code: `crates/cyber-workflow` (bundled modules, `Origin::Bundled`, discovery, the `tools` host API), `crates/cyber-server` (`runtime/workflow_host.rs` direct tools and the review preparation step, `http/workflows.rs` library route, openapi), `crates/cyber-tools/src/tools/workflow.rs` (description, approval estimate), `crates/cyber-cli/src/commands/workflows.rs` (`list` origin column, `show`, `eject`), `crates/cyber-cli/src/commands/review.rs` and the TUI `/review` path, `crates/cyber-tui` (Library tab), the TS SDK (`client.workflows.library()`).
- No new dependencies. No storage migration: runs record the module digest and origin as today.
- Behavior change: `/review` and `cyber review` become multi-agent and cost more. The review's default budget is small (at most 24 agents, $2), small diffs use one lens pass over the whole patch, and a project that wants the P1 single-agent review keeps it by adding a `review` skill or command, which already overrides the bundled path.
