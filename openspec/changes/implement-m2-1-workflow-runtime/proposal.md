## Why

ROADMAP P2 milestone M2.1 ("Workflow runtime: QuickJS sandbox, `agent()/parallel()/pipeline()/phase()`, determinism guards, event-sourced resume, budgets, model tiers `fast`/`smart`/`verify`") has no implementation. Only placeholders exist: `cyber workflows` is registered but unavailable, the `workflows` config key is accepted opaquely, `/workflows` is a reserved command, the hook event names are accepted, `budgets.run.max_agents` is validated, and `Admission.source` documents `workflow`. The `workflows` spec defines 26 P2 requirements, and related requirements in other capabilities depend on Workflow Runs.

The last P1 change exercised features only through test hosts, not through the production `AppHost`. This change therefore requires every increment to have at least one test through a production path: the real `cyber` binary, the App HTTP router, or `cyber exec`.

## What Changes

The milestone ships in four increments. Each one is a working slice tested through a production path.

1. **Engine, discovery, config and surface stubs (this increment).**
   - A new crate, `cyber-workflow`, holds the module format, static pure-literal `meta` extraction, forbidden-import detection, saved-Workflow discovery (project overrides global), typed diagnostics, and a sandboxed QuickJS engine. The engine is `rquickjs` pinned to `=0.14.0` and has a heap limit, an interrupt-based script CPU limit, a frozen determinism prelude and no code generation. It talks to a `WorkflowHost` trait.
   - The `workflows` config section is validated.
   - `cyber workflows list` works; the other subcommands are explicit "not available yet" errors.
   - `/api/v1/workflows/*` answers `503 ServiceUnavailableError {service: "workflow"}`.
2. **Durable run model and resume.**
   - Runs become a new `run_` aggregate with `workflow.run.*.1` lifecycle events, `workflow.agent.settled.1` and `workflow.call.recorded.1`, plus projections, a migration and startup recovery.
   - Replay keys are `sha256(prompt + canonical options)` plus an ordinal, together with the script and config digests of each revision. A divergent replay fails with `WorkflowReplayMismatch`.
   - Server shutdown marks running runs `paused`.
3. **Agent spawning and run policy.**
   - `agent()` runs real child Sessions through the existing subagent/Job machinery. It uses a `ToolHost` workflow method beside `subtask_request`, idempotent operation keys, schema-validated structured output, the global FIFO concurrency cap, model tiers, run budgets, the write-isolation default (a worktree for writers), the `workflow.run` permission and run Mode ceilings.
   - Non-interactive `ask` is auto-rejected, and run results are handed back to the starting Session.
4. **Surfaces.**
   - Background execution on the server, with `/api/v1/workflows` routes, OpenAPI and the generated SDK `client.workflows`.
   - The full `cyber workflows run/runs/show/pause/resume/stop/logs` CLI with `--wait` exit codes.
   - `cyber exec --workflow`, the model-facing `workflow` tool, and the minimal `/workflows` TUI monitor.

The brief named the commands `run/list/status/resume/cancel`. They map onto the spec's CLI as `run`, `list`, `show` (status), `resume` and `stop` (cancel). The spec also adds `runs`, `pause` and `logs`.

## Spec corrections (deltas)

- **workflows / Workflow module format.** Scripts are JavaScript ES modules. TypeScript is deferred until a type-strip step is chosen, because swc's dependency weight conflicts with the disk and cross-compilation budget. Discovery reports `.ts` modules as `UnsupportedSyntax`.
- **workflows / Sandboxed QuickJS runtime.**
  - The CPU limit is measured as wall time of synchronous JavaScript slices. Thread CPU clocks are not used.
  - Garbage-collection observers and shared-memory blocking are also removed.
  - The config ranges are now specified.
- **workflows / Diagnostics.** Adds `WorkflowReplayMismatch`, which the Replay requirement already uses. A memory-limit failure is reported as `ExecutionFailure: memory limit of N MiB exceeded`.
- **workflows / Model tiers and mixing.** A tier's value is a model reference or `role:<role>`. Tier names are open, matching provider-catalog, and use the pattern `^[a-z][a-z0-9_-]{0,31}$`.
- **workflows / Run monitor.** The stop key is `x`, as in the tui Workflow monitor (previously `s`).
- **worktrees / Workflow agent isolation.** `run_<id>-<index>` violates the managed worktree name pattern, which has no underscores and no uppercase. Workflow agent worktrees are now named `run-<lowercase run ULID>-<index>` on branch `cyber/run-<lowercase run ULID>/<index>`.

## Requirement mapping

The tasks are in `tasks.md`. "Inc" is the increment that delivers each requirement.

| Capability / Requirement | Inc | Tasks |
|---|---|---|
| workflows / Workflow module format | 1 | 2.1, 2.2 |
| workflows / Sandboxed QuickJS runtime | 1 | 2.3, 2.4 |
| workflows / Host API surface | 1 (globals), 3 (`tools`) | 2.5, 4.9 |
| workflows / agent() call | 3 | 4.1–4.4 |
| workflows / Parallel and pipeline helpers | 1 (helpers), 3 (global cap) | 2.5, 4.5 |
| workflows / Phases, logs and checkpoints | 1 (engine), 2 (durable) | 2.5, 3.4 |
| workflows / ask() user interaction | 3 | 4.8 |
| workflows / Direct read-only tools | 3 | 4.9 |
| workflows / Determinism rules | 1 | 2.4 |
| workflows / Durable agent results and resume | 2 | 3.1–3.5 |
| workflows / Run lifecycle states | 2 | 3.2, 3.6 |
| workflows / Background execution on the server | 2, 4 | 3.7, 5.1 |
| workflows / Concurrency limit | 1 (config), 3 | 2.7, 4.5 |
| workflows / Run budgets | 3 | 4.6 |
| workflows / Model tiers and mixing | 1 (config), 3 | 2.7, 4.7 |
| workflows / Prompt-cache warm-up for fan-out | 3 | 4.11 |
| workflows / Write isolation default | 3 | 4.10 |
| workflows / Permissions for runs | 3 | 4.12 |
| workflows / Saved and bundled workflows | 1 (saved discovery and override) | 2.6 |
| workflows / Model-authored workflows | 4 | 5.5 |
| workflows / Run results | 3, 4 | 4.13, 5.1 |
| workflows / Diagnostics | 1 (types), 2–4 (sources) | 2.2, 3.5, 4.x |
| workflows / Run monitor | 4 | 5.1, 5.6 |
| workflows / Workflow CLI | 1 (`list`), 4 | 2.8, 5.3 |
| workflows / Replay of workflow host calls | 2 | 3.3–3.5 |
| background-tasks / Job registry, Tasks view, Concurrency limits | 4 | 5.7 |
| agents-subagents / Concurrency cap, Cost attribution | 3 | 4.5, 4.6 |
| worktrees / Workflow agent isolation | 3 | 4.10 |
| permissions-modes / Non-interactive behavior (workflow agents), Default rules (`workflow.run`) | 3 | 4.12 |
| observability-costs / Budgets, Cost rollups (`scope=run`) | 3, 4 | 4.6, 5.1 |
| observability-costs / Status line data contract (running workflows) | 4 | 5.6 |
| server-api / Route groups (`workflow` 503, then routes) | 1, 4 | 2.9, 5.1 |
| server-api / Health and version (capability discovery) | 4 | 5.1 |
| client-sdk / Workflow helpers, Browser-safe schemas (`WorkflowRun`), Documented examples | 4 | 5.2 |
| cli-commands / Command tree, Resource command groups (`workflows`) | 1, 4 | 2.8, 5.3 |
| cli-commands / child env `CYBER_WORKFLOW_RUN_ID` (Resource command groups) | 3 | 4.3 |
| exec-mode / Workflow exec, Stream JSON event schema (`workflow_progress`), Command and prompt assembly | 4 | 5.4 |
| hooks / Supported events (workflow events), Hooks in subagents and workflows | 3 | 4.14 |
| skills-commands / Built-in commands (`/workflows`), Command source precedence (reserved) | 4 | 5.6 |
| tui / Workflow monitor, Background tasks view | 4 | 5.6 |
| compaction / Compaction in subagents and workflows | 3 | 4.15 |
| goals / Deferred evaluation during background work | 4 | 5.8 |
| session-runtime / Durable prompt admission (`source: workflow`) | 3 | 4.13 |
| storage-events / Durable event store, Core tables (`workflow_run`, `workflow_agent`) | 2 | 3.1, 3.2 |
| storage-events / Session export, Retention and garbage collection (workflow parts) | 4 | 5.9 |
| provider-catalog / Model reference format, Model roles (tiers) | 3 | 4.7 |

## Out of scope

- **workflows / Saved and bundled workflows:** the bundled `review`, `audit`, `migrate`, `research` and `plan` modules (M2.2), and plugin-contributed Workflows (plugins-marketplace, P4).
- **workflows / Automatic orchestration:** `workflows.auto` behavior and `/orchestrate`. The config keys are validated now; the behavior is a later P2 milestone.
- **goals / Goals on workflow agents and subagents:** the `agent({ goal })` option (M2.3 goals). Until then the option is rejected with `ExecutionFailure`.
- **workflows / Remote execution on runners** and **Remote agents** (P3, runners-cloud), including `RunnerUnavailable` and `--runner`.
- **workflows / Workflow sharing** (P4) and plugin-contributed workflows (plugins-marketplace).
- **skills-commands:** bundled `review`/`audit` skills built on workflows (M2.2).
- OpenTelemetry workflow spans (observability-costs / OpenTelemetry export) beyond the existing session spans, the remote-control push notifications for runs, routines that start workflows, and vcs-integration jobs (later phases).

## Impact

- **New crate:** `crates/cyber-workflow`, with a new pinned dependency `rquickjs =0.14.0` (MIT; compiles quickjs-ng through `cc` with pre-generated bindings for every release target).
- **Changed code:**
  - `cyber-core` (`config/workflows.rs`, validation).
  - `cyber-cli` (`workflows` command, tests).
  - `cyber-server` (`http/workflows.rs`).
  - `cyber-app` (tests).
  - Later increments change `cyber-server/runtime`, `cyber-store` migrations, `cyber-tools`, the SDK and `cyber-tui`.
- **Evidence:** `docs/implementation/p2-status.md`.
