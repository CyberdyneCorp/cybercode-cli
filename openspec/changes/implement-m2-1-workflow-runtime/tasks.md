## 1. OpenSpec change

- [x] 1.1 Map every M2.1 `workflows` requirement and the related background-tasks, agents-subagents, worktrees, permissions-modes, observability-costs, server-api, client-sdk, cli-commands, exec-mode, hooks, skills-commands, tui, compaction, goals, storage-events, session-runtime and provider-catalog requirements to tasks or to out of scope (`proposal.md`).
- [x] 1.2 Record the choice of JS engine (rquickjs `=0.14.0` over boa), the static meta parser (a hand-written scanner over oxc), the TypeScript deferral and the scheduler design (`design.md`).
- [x] 1.3 Add spec deltas for the real conflicts:
  - module format (TypeScript deferred)
  - sandbox limits and the removed globals
  - the `WorkflowReplayMismatch` diagnostic
  - tier values
  - monitor stop key `x`
  - worktree naming `run-<ulid>-<index>`
- [x] 1.4 `openspec validate --all --strict` passes.

## 2. Engine, discovery and surface stubs (increment 1)

- [x] 2.1 New crate `crates/cyber-workflow` with `rquickjs =0.14.0` (default features off).
- [x] 2.2 Typed `Diagnostic { kind, message, position }`, covering all 11 kinds. Static module checks:
  - the 1 MiB size limit
  - forbidden `import`, `export … from` and `require` (reported as `UnsupportedSyntax` with the specifier and `line:col`)
  - pure-literal `meta` (reported as `ParseError: meta must be a pure literal` at the offending token)
  - `meta` field validation
  - a `sha256:` digest
- [x] 2.3 Per-run Runtime and Context:
  - heap limit (`max_memory_mb`)
  - interrupt handler that charges the `Instant` time of synchronous JS slices against `script_cpu_seconds` (`Timeout`)
  - no loader, so imports fail at link time as well
  - memory exhaustion reported as `ExecutionFailure: memory limit of N MiB exceeded`
- [x] 2.4 Frozen determinism prelude:
  - `Date.now`, `Math.random`, `performance.now`, `Date()` and argument-less `new Date()` throw the determinism message
  - `new Date(value)` still works
  - `eval`, `Function`, the Function-constructor back doors, timers, `WebAssembly`, `WeakRef`, `FinalizationRegistry`, `SharedArrayBuffer` and `Atomics` are removed
- [x] 2.5 Host API globals: `agent`, `ask`, `checkpoint`, `phase`, `log`, `parallel` (`concurrency`, `settle`), `pipeline`, and the frozen `args` and `ctx`.
  - The cooperative scheduler runs host futures through the `WorkflowHost` trait.
  - Typed host errors are catchable and cannot be forged.
  - Agent options are validated.
  - The log is capped at 1 MiB with a marker.
  - A promise that can never settle is detected.
- [x] 2.6 Saved-Workflow discovery:
  - global `<config>/workflows`, then project `.cyber/workflows` from the root down to the Location
  - `meta.name` overrides, with `shadowed` paths
  - bounded traversal that never follows links
  - `.ts` reported as `UnsupportedSyntax`
  - inline scripts supported through `WorkflowModule::inline`
- [x] 2.7 `workflows` config section in cyber-core: `max_concurrent`, `max_memory_mb`, `script_cpu_seconds`, `tiers` (model reference or `role:<role>`), `auto`, `auto_threshold_files` and `size_guideline`. These are validated during config load and replace the opaque key.
- [x] 2.8 `cyber workflows list` (table and JSON) through the real binary. `run`, `runs`, `show`, `pause`, `resume`, `stop` and `logs` are explicit "not available yet" errors.
- [x] 2.9 `/api/v1/workflows` and `/api/v1/workflows/*` answer `503 ServiceUnavailableError {service: workflow}` behind authentication on the production App router.
- [x] 2.10 Evidence in `docs/implementation/p2-status.md`. The windows-gnu clippy check passes for `cyber-workflow`, `cyber-cli`, `cyber-core` and `cyber-server`.

## 3. Durable run model and resume

Increment 2 delivered the runtime core (section 6) ahead of resume, so 3.1 is done and 3.2–3.4 are partly done; their remaining parts move to increment 3.

- [x] 3.1 `cyber-store` migration for the `workflow_run` and `workflow_agent` tables, and the `run_` aggregate in `runtime/events.rs` `registry()`, with projectors.
- [x] 3.2 Lifecycle events `workflow.run.{started,paused,resumed,completed,failed,stopped}.1`, and statuses `queued`, `running`, `waiting_input`, `paused`, `completed`, `failed`, `budget_exceeded` and `stopped`. (Increment 3: all events; `queued` and `waiting_input` are reserved until `ask()` lands.)
- [x] 3.3 Replay keys: `sha256(prompt + canonical options)` plus an ordinal per key. Each revision records the script and config digests. (Increment 3: checkpoints are keyed by name; revisions record the source, script digest and configuration digest.)
- [x] 3.4 `workflow.agent.settled.1` and `workflow.call.recorded.1` (checkpoint, ask, tools) are persisted before the promise resolves. Phase and log events are recorded too. (Increment 3: `workflow.call.recorded.1` records checkpoints, reused results and budget rejections; `ask()` and `tools` record through it when they land, 4.8 and 4.9.)
- [x] 3.5 Relaunch re-executes the script and resolves recorded calls from the store. A divergent sequence fails with `WorkflowReplayMismatch`. Unsettled agents resume their existing child Session before any replacement is spawned.
- [x] 3.6 Pause lets in-flight agents finish. Stop cancels them. A relaunch is refused while cancelled agents are still exiting.
- [x] 3.7 Process-local Control map. `recover_workflow_runs` runs in `App::build`. Shutdown marks running runs `paused` (`runtime/shutdown.rs`).

## 4. Agent spawning and run policy

- [x] 4.1 `ToolHost::spawn_workflow_agent` (default: rejects with `AgentFailed`, unsupported), implemented in `BuiltinHost` next to `subtask_request`, with an idempotent `operation_key` from the run and call key. `AppHost` forwards it.
- [x] 4.2 `agent()` resolves to `{ text, session_id, cost, tokens }` and gets an `agt_` ID.
- [ ] 4.3 Child Sessions carry `CYBER_WORKFLOW_RUN_ID` in tool child processes. (Increment 2: child Sessions carry `workflow_run_id`; the environment variable remains.)
- [x] 4.4 A `schema` is validated, and a mismatch is reported as `SchemaMismatch` after a single result retry. A provider error after retries is reported as `AgentFailed`.
- [x] 4.5 Global FIFO cap `min(max_concurrent, cores)` across runs, which `parallel({ concurrency })` can only narrow. Workflow agents are excluded from `agents.max_concurrent`.
- [x] 4.6 Run budgets: the observability-costs Budget plus `max_agents`, from `meta`, the invocation or `budgets.run`. Exceeding one gives `BudgetExceeded: <budget>` and the `budget_exceeded` status. `ctx.budget` is live. Child costs roll up.
- [x] 4.7 Model tiers resolve through `model_roles` (`verify` falls back to the default model). An unknown tier is reported as `ModelUnavailable`. Cost is reported per model. (Increment 3: `GET /usage?scope=run` reports cost per agent, model and tier.)
- [ ] 4.8 `ask()`: an interactive run moves to `waiting_input` through the question flow. A non-interactive run without a pre-declared answer gets `NoInteractiveUser`.
- [ ] 4.9 The `tools.read`, `tools.glob` and `tools.grep` allowlist, with the built-in permission checks and limits. Results are recorded for replay.
- [x] 4.10 Write isolation: concurrent writers in one Location default to a worktree named `run-<ulid>-<index>` on branch `cyber/run-<ulid>/<index>`, and the result includes `branch`.
- [ ] 4.11 Prompt-cache warm-up for `parallel` groups that share a cacheable prefix.
- [x] 4.12 The `workflow.run` permission defaults to `ask` in `default` Mode and `allow` in `auto` and `bypass`. The run's Mode is a ceiling for its agents. In a non-interactive run, an agent's `ask` is auto-rejected with `No interactive approver for workflow run`.
- [x] 4.13 Run results: the return value (up to 1 MiB, else a file with a preview), and a summary admitted to the starting Session with `delivery: queue` and `source: workflow`.
- [x] 4.14 Hooks fire for workflow agent tool calls with `workflow_run_id`, plus the workflow hook events.
- [ ] 4.15 Compaction rules apply to workflow agent Sessions and are reported to the monitor.
- [ ] 4.16 Follow-up (P2 exit criterion, not M2.1-blocking): `reserved` budget enforcement. `crates/cyber-server/src/runtime/budget.rs` `validate` refuses it today with "Reserved Session budget enforcement is not available"; run budgets ship with `soft`/`hard` enforcement until reservations land.

## 5. Surfaces (increment 4)

- [ ] 5.1 Background execution on the server, and `/api/v1/workflows` routes for listing saved Workflows and for runs (start, list, show, pause, resume, stop, logs and live events). OpenAPI, capability discovery, and `scope=run` usage. (Increment 2: background execution and run start, list and show. Increment 3: pause, resume, stop, logs and `scope=run` usage. Increment 4: OpenAPI `v1.workflow.*` and live run events on the instance event stream. Saved-Workflow listing over HTTP and capability discovery remain.)
- [x] 5.2 The generated TS SDK `client.workflows.run()` handle (`events`, `status`, `pause`, `resume`, `cancel`, `result`), the `WorkflowRun` schema and an example.
- [x] 5.3 `cyber workflows run/runs/show/pause/resume/stop/logs`. `run --wait` exits with 0 for completed, 1 for failed, 4 for `BudgetExceeded` and 130 when interrupted.
- [ ] 5.4 `cyber exec --workflow <name|path> --args` with `workflow_progress` stream events and exit codes.
- [ ] 5.5 The model-facing `workflow` tool (a name or an inline script). The inline script is persisted to `<state>/workflows/<run_id>.js` and can be saved into `.cyber/workflows/`. (Increment 4: the tool, the permission and the persisted `script_file`; the Save action arrives with the TUI monitor, 5.6.)
- [ ] 5.6 Minimal `/workflows` TUI monitor: `p` pauses or resumes, `x` stops, `r` relaunches, `t` opens a transcript. It also shows a progress line and the status-line count of running workflows.
- [ ] 5.7 Background-tasks integration: Job kind `workflow`, and runs listed in `/tasks`.
- [x] 5.8 Goal evaluation is deferred while runs started by the Session are active.
- [ ] 5.9 Session export includes workflow run summaries. Retention of finished agent transcripts is applied.

## 6. Runtime core (increment 2)

- [x] 6.1 Fallible `WorkflowHost::phase` and `log`, and `WorkflowHost::fatal`, so a record that cannot be written fails the run before the script observes the outcome.
- [x] 6.2 `crates/cyber-server/src/runtime/workflows.rs`: the `run_` aggregate, the `workflow_run` and `workflow_agent` projections (migration `20261016000000_workflow_runs`), the events `workflow.run.{started,completed,failed}.1`, `workflow.agent.settled.1`, `workflow.phase.1` and `workflow.log.1` registered in `events::registry`, and `Runtime::start_workflow_run`, `workflow_runs`, `workflow_run` and `workflow_agents`.
- [x] 6.3 The engine runs on a dedicated thread with a local executor. Host calls cross to the tokio runtime, and dropping a pending call (the run ended) cancels its child.
- [x] 6.4 `runtime/workflow_host.rs`: agent option checks (`goal`, `tools`, `budget` and `isolation: "worktree"` reject as not available), schema validation, tier resolution, call keys, the server-wide FIFO semaphore (`Runtime::configure_workflow_concurrency`, set by `App::build` from the global configuration), and the settled record appended before the result is delivered.
- [x] 6.5 `agent.rs` spawns every child through one path keyed by `SpawnSource` (`Model`, `User` or `Workflow`). Workflow agents are foreground children of the owner Session with `workflow_run_id`, the `agt_` ID as subagent name, deny-only delegation checks, the run Mode as ceiling through `child_mode`, and a permit from the Workflow limit that `ChildGuard` holds until the child settles.
- [x] 6.6 `POST /api/v1/workflows/runs` (owner `session_id`, saved `name` or inline `script`, `args`), `GET /api/v1/workflows/runs[?session_id=]` and `GET /api/v1/workflows/runs/{id}` with the settled agents. Other workflow paths still answer `503` naming the group.
- [x] 6.7 Tests: engine group concurrency and fail-closed host records; server runtime runs with a fake agent host (ordering of settled records, ordinals, FIFO across runs, tiers, unavailable options, unsupported hosts); `BuiltinHost` agents on scripted models (child identity and Mode, single schema retry, typed failures, `agents.max_concurrent` not applied); the production App over HTTP with a local OpenAI-compatible model.

## 7. Replay, lifecycle and run budgets (increment 3)

- [x] 7.1 `runtime/workflow_replay.rs`: same-revision relaunches deliver recorded results in their recorded order, and the prefix ends at the first failure. Divergence (a different key at a sequence number, or a recorded result whose call is never issued) fails with `WorkflowReplayMismatch`. After the prefix, calls match by key: reuse, resume the child, or spawn. Replayed logs and phases are not recorded again.
- [x] 7.2 Revisions (`workflow.run.revised.1`, `workflow_run_revision`): a changed script or configuration digest needs `revision: true`.
- [x] 7.3 `checkpoint(name, value)` is durable (`workflow.call.recorded.1`) and returns its recorded value on replay.
- [x] 7.4 `runtime/workflow_control.rs`: process-local `Control` with signal and leases; `pause`, `stop` (with `stop_subtree` of unsettled children), `resume` (refused while agents exit), `recover_workflow_runs` in `App::build`, and pausing in `Runtime::shutdown` before `tools.shutdown`.
- [x] 7.5 Unsettled agents resume their child Session through the shared agent path (`WorkflowAgentRequest.resume`). A stopped child is reopened with its receipt, and the child's Drain reconciles interrupted calls.
- [x] 7.6 Run budgets: `RunBudget` layering and defaults, the `workflow_run_budget` scope in `runtime/budget.rs` through `session.workflow_run_id`, `check_run` before each new agent, `budget.exceeded.1` on the run, `budget_exceeded` status, and live `ctx.budget`.
- [x] 7.7 `GET /api/v1/usage?scope=run` (`RunUsageReport`, discriminated `ScopeUsage`), and `POST /workflows/runs/{id}/pause|resume|stop` and `GET …/logs`.
- [x] 7.8 Tests: replay units, server runtime lifecycle and budget tests (fake host and real child Sessions), the production App restarted on the same database file over HTTP, HTTP stop/resume with a real child, `max_agents` over HTTP, per-model run usage, and the Windows service shutdown and recovery hook.

## 8. Surfaces and safety (increment 4)

- [x] 8.1 `workflow.run` default rule (`ask`, source `default`, visible through `evaluate`) allowed in `auto` and `bypass`; a configured rule replaces it. The approval carries name, phases, budget and origin.
- [x] 8.2 Non-interactive runs (HTTP and CLI) resolve every ask of their agents and the agents' subagents as unattended (`requests.rs`), reported as `No interactive approver for workflow run`; runs started by the `workflow` tool in an attended Session route asks to its clients.
- [x] 8.3 Write isolation: `ToolHost::workflow_agent_writes` (AppHost forwards it), concurrent edit-capable agents of a git checkout get `run-<ulid>-<index>` on `cyber/run-<ulid>/<index>` through `create_child_worktree`, deny-only and kept until prune; results and agent records carry `branch`.
- [x] 8.4 Run results: inline up to 1 MiB, else `<state>/workflows/<run_id>.result.json` with a preview; one queued `msg_workflow_<run_id>` summary (source `workflow`, `workflow.run.notified.1`), re-admitted idempotently by startup recovery.
- [x] 8.5 The `workflow` tool (`name` or `script`, `args`, `budget`) behind `workflow.run`; inline scripts persisted at `<state>/workflows/<run_id>.js`; refused inside workflow agents.
- [x] 8.6 `cyber workflows run/runs/show/pause/resume/stop/logs` through `cyber-client` against the shared server; `--wait` exits 0/1/4/130; `--runner` is rejected as unavailable.
- [x] 8.7 OpenAPI `v1.workflow.*`, regenerated `sdk/openapi.json` and TS sources, the handwritten `client.workflows.run()` handle, its tests and `examples/run-workflow.ts`.
- [x] 8.8 Run events published on the bus and mapped to the run's Location in `event_location.rs`.
- [x] 8.9 `WorkflowRunStart`/`WorkflowRunEnd` hooks on the owner Session; `workflow_run_id` in `HookIdentity` (reserved, `run_` prefix) for those events and for workflow agents' Sessions.
- [x] 8.10 Tests: permission engine unit; server runtime (isolation decisions, results, exactly-once handback across a restart); native git worktree isolation in `cyber-tools`; the production App (tool approval and handback, deny, unattended asks, SSE, hooks); the real `cyber` binary against the shared server with a scripted model; SDK handle tests. The Windows job runs the workflow suites.


## 9. End-to-end acceptance with the real binary

- [x] 9.1 The debug `cyber` binary against a scripted local provider: saved fan-out under `workflows.max_concurrent`, an inline run over HTTP in the background, determinism guards, resume after `kill -9` of the server, a token budget ending with exit 4, stop, `workflow.run` deny rules and Mode ceilings (evidence in `docs/implementation/p2-status.md`).
- [x] 9.2 Child-ownership claims record the `server.lock` holder (`Runtime::adopt_server_lock`), and the next holder releases claims of an exited server, so a crashed run resumes. Tests: `child_ownership::the_next_server_lock_holder_releases_only_claims_of_an_exited_server`, `cli::a_run_resumes_after_the_server_is_killed_mid_agent` (unix).
- [x] 9.3 Agents in flight when the script ends are recorded `cancelled` on `workflow.run.completed.1` and `workflow.run.failed.1`. Test: `workflow_runs::agents_in_flight_when_the_script_ends_are_recorded_cancelled`.
- [x] 9.4 `run --wait` progress tracks agents by ID in phase order, and errors name their kind once. Unit tests in `commands::workflows`.
