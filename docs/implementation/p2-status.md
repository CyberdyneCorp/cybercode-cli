# P2 implementation status

P2 evidence lives here and is kept separate from the P1 files (`p1-status.md`, `p1-handoff.md` and `p1-acceptance-audit.md`). The plan, the requirement-to-task mapping and the scope exclusions are in [`openspec/changes/implement-m2-1-workflow-runtime`](../../openspec/changes/implement-m2-1-workflow-runtime/proposal.md).

## Provider-reported usage limits (`improve-tui-experience` section 9, 2026-10-16)

Uncommitted, part of the TUI overhaul working tree (see the P1 status entry of the same date for the whole-tree validation). `cyber-llm/src/limits.rs` reads plan and rate-limit windows from provider response headers (Anthropic unified and rate-limit headers, `x-codex-*`, `x-ratelimit-*`), never from local usage; `GET /api/v1/usage/limits` (`v1.usage.limits`, SDK `client.usage.limits()`) lists the latest reading per provider and window, and the TUI Usage tab draws one meter per window with `d`/`w`. A provider without such headers yields an empty list and no meters (tested through the App router in `crates/cyber-app/tests/usage_limits.rs`). Not driven against a live provider.

## M2.1 Workflow runtime, increment 1 of 4: engine, discovery, config and surface stubs (2026-10-15)

### OpenSpec

The change `implement-m2-1-workflow-runtime` has a proposal, design, tasks and spec deltas:

- The proposal maps all 26 P2 `workflows` requirements, plus the related requirements in background-tasks, agents-subagents, worktrees, permissions-modes, observability-costs, server-api, client-sdk, cli-commands, exec-mode, hooks, skills-commands, tui, compaction, goals, storage-events, session-runtime and provider-catalog, either to tasks across four increments or to an out-of-scope list:
  - M2.2 bundled workflows
  - automatic orchestration and `/orchestrate`
  - the M2.3 `goal` option
  - P3 runners and remote agents
  - P4 sharing
- The deltas resolve the real conflicts:
  - TypeScript is deferred; `.ts` files are listed as `UnsupportedSyntax`.
  - The sandbox limits are specified as charged synchronous JS time, with ranges and the extra removed globals.
  - `WorkflowReplayMismatch` is added to Diagnostics, and forged kinds are reported as `ExecutionFailure`.
  - The tier value format is defined.
  - The monitor stop key is `x`, as in tui.
  - Workflow worktrees are named `run-<ulid>-<index>` on `cyber/run-<ulid>/<index>`, because `run_<id>-<index>` violated the managed-name pattern.

`openspec validate --all --strict` passes for 63 items. `spec_lint.py` reports 0 errors and the same 21 warnings as before.

The brief named the commands `run/list/status/resume/cancel`. They map onto the spec's `cyber workflows run/list/show/resume/stop`; the spec also has `runs`, `pause` and `logs`.

### Engine (`crates/cyber-workflow`)

**JS engine.** The engine is rquickjs, pinned to `=0.14.0` with default features off.

- License: MIT.
- Pre-generated bindings cover every release target (darwin, linux gnu and musl, windows msvc and gnu).
- The C code builds through `cc`, as with `rusqlite` `bundled`.
- New transitive packages: `rquickjs-core`, `rquickjs-sys` and `relative-path` (MIT OR Apache-2.0).
- boa was rejected because it has no heap limit or interrupt handler.

**Meta parser.** A hand-written tokenizer is used instead of oxc, because of disk and build weight. It extracts a pure-literal `meta` and detects `import`, `export … from` and `require`.

**Per-run sandbox.** Each run gets its own Runtime and Context:

- **Heap limit.** Exhaustion is reported as `ExecutionFailure: memory limit of N MiB exceeded`.
- **CPU limit.** An interrupt handler charges only synchronous JS slices, measured with `Instant`, against `script_cpu_seconds`. Exceeding it gives `Timeout`. Time awaiting host calls is not charged.
- **No module loader.**
- **Frozen prelude.**
  - `Date.now`, `Math.random`, `performance.now`, `Date()` and argument-less `new Date()` throw the determinism message. `new Date(0)` still works.
  - `eval`, `Function`, the Function-constructor back doors, timers, `WebAssembly`, `WeakRef`, `FinalizationRegistry`, `SharedArrayBuffer` and `Atomics` are removed.
  - The host API globals (`agent`, `ask`, `checkpoint`, `phase`, `log`, `parallel`, `pipeline`, `args`, `ctx`) are non-writable. `args` and `ctx` are deep-frozen.

**Scheduler.** A cooperative scheduler runs host futures through the `WorkflowHost` trait. Host errors carry a `kind` that scripts can catch but cannot forge. Agent options are validated against the documented keys. The log is capped at 1 MiB with a marker. A promise that can never settle is detected, and at most 10,000 calls may be pending.

**Diagnostics.** A typed `Diagnostic` covers all 11 kinds and carries `line:col`.

**Discovery.** Saved Workflows are discovered from:

1. `<config>/workflows`
2. each `.cyber/workflows` from the project root down to the Location

A later root overrides an earlier one by `meta.name`, and the replaced paths are recorded as `shadowed`. Traversal is bounded, never follows links, and reads files of at most 1 MiB. Inline scripts load through `WorkflowModule::inline`.

### Config (`cyber-core`)

`config/workflows.rs` validates the `workflows` section during config load, replacing the opaque key:

| Key | Default | Range or values |
|---|---|---|
| `max_concurrent` | 16 | 1–256; the effective value is `min(config, cores)` |
| `max_memory_mb` | 256 | 16–4096 |
| `script_cpu_seconds` | 60 | 1–3600 |
| `tiers` | defaults `fast`/`smart`/`verify` | name pattern `^[a-z][a-z0-9_-]{0,31}$`; value `provider/model` or `role:<role>` |
| `auto` | `suggest` | `off`, `suggest`, `on` |
| `auto_threshold_files` | 10 | 1–100,000 |
| `size_guideline` | 10 | 1–1000 |

Unknown keys are rejected.

### Production surfaces

- **CLI.** `cyber workflows list` (table and `--format json`) discovers saved Workflows for the Location and honours `CYBER_DISABLE_PROJECT_CONFIG`. `run`, `runs`, `show`, `pause`, `resume`, `stop` and `logs` are parsed and exit 2 with "`cyber workflows <sub>` is not available in this build yet".
- **HTTP.** `ANY /api/v1/workflows` and `/api/v1/workflows/{*rest}` answer `503 {"_tag":"ServiceUnavailableError","service":"workflow"}` behind the normal origin and authentication guard. Before this change they returned an untagged 404 even without credentials. These routes are not in OpenAPI yet, so the SDK is unchanged.
- **CI.** A native Windows step, "Native workflow engine, discovery and surfaces", runs the `cyber-workflow`, CLI and app workflow tests.

### Tests (red first where noted)

**`cyber-workflow`:** 16 unit tests and 13 engine tests that use a fake host.

Unit tests:

- tokenizer: strings, templates and regexes
- pure-literal meta and the offending-token positions
- forbidden imports with specifier and position
- discovery: override, `.ts`, parse errors and links (Unix)

Engine tests:

- agents, phases, logs and checkpoints reaching the host
- top-level await
- frozen globals
- undefined capabilities (`fetch is not defined` with its line)
- determinism guards
- busy-loop and post-await `Timeout`, and a slow host that is not charged
- 16 MiB memory limit
- `ParseError` and thrown-error positions
- catchable `AgentFailed`, a forged kind reported as `ExecutionFailure`, and `ask` reported as `NoInteractiveUser`
- invalid options, a promise that never settles and a cyclic return value
- `parallel` order, concurrency and settle mode
- a pipeline whose second stage starts before slow first-stage items finish
- log truncation

These tests were written together with the module, so they have no recorded red run.

**`cyber-core` `config`:** `workflow_settings_have_defaults_and_validate_ranges_and_tiers`. It was red first (the types were missing).

**`cyber-cli` `cli`** (red first, against "`cyber workflows` is not available"):

- `workflows_list_shows_saved_workflows_with_project_overrides_and_parse_errors` runs the real binary in a temp repository with global and project modules. It expects names, scopes and descriptions, the project override, `computed.js: ParseError: meta must be a pure literal (2:29)`, and the JSON shape.
- `workflow_run_subcommands_are_explicitly_unavailable`.
- `unshipped_command_is_explicitly_unavailable` now uses `goals`.

**`cyber-app` `workflows`** (red first, an untagged 404): `workflow_routes_answer_service_unavailable_naming_the_group` uses the real App router over authenticated TCP. It expects 401 without credentials and 503 with `service: workflow` for GET, POST and DELETE on the group.

### Verification (macOS)

Results:

- **Pass:**
  - `cargo fmt --check`.
  - Native clippy with `-D warnings` for `cyber-workflow`, `cyber-core`, `cyber-cli`, `cyber-server` and `cyber-app` (all targets).
  - `x86_64-pc-windows-gnu` clippy for the same crates (all targets, stub MinGW CC/AR, type-check only).
  - `cyber-workflow` (29 tests), `cyber-core` `config` (51) and `ci_contracts`.
  - `cyber-cli`: every test binary except `review_command` (below).
  - `cyber-server` `http` (62).
  - `cyber-app` `workflows`.
- **Pass in isolation only:** `cyber-app` `server` passed 42 of 43. `public_worktree_creation_streams_setup_and_retains_failed_session` hit its 10 s setup-event timeout under full parallel load, and passes alone.
- **Flaky under parallel load:** `cyber-cli` `review_command`. One or two tests fail intermittently because of their 2 s billing-refresh and 3 s review timeouts. The suite passes with `--test-threads=1`. A `main` worktree that shared the target directory also failed in 2 of 8 parallel runs, against 6 of 9 on this branch, so the flake predates this change. The runs overwrote each other's `target/debug/cyber`, so these numbers are only indicative.

### Status and next increments

M2.1 is **in progress**. Increment 1 delivers static loading and a sandboxed engine but does not execute runs. The next increments are:

- **Increment 2: durable model and resume.**
  - `run_` aggregate, events, migration and projections.
  - Replay keys and revisions, and `WorkflowReplayMismatch`.
  - Pause, stop and recovery; shutdown marks runs `paused`.
- **Increment 3: real agents and run policy.**
  - Real child-Session agents through a `ToolHost` workflow method.
  - Schema validation, concurrency cap, budgets, tiers, write isolation, permissions and Mode ceiling, `ask`, read-only `tools`, and result handback.
- **Increment 4: surfaces.**
  - Server routes, OpenAPI and SDK.
  - Full CLI with `--wait` exit codes, `cyber exec --workflow`, the `workflow` tool, and the `/workflows` TUI monitor.

Reserved budget enforcement is still refused by `crates/cyber-server/src/runtime/budget.rs`. It is a P2 exit criterion but does not block M2.1, so it is tracked as follow-up task 4.16.

Re-verified on 2026-10-15 after the interrupted run: `cargo test --locked` passes for `cyber-workflow` (29), `cyber-core` `config` (51) and `ci_contracts`, `cyber-cli` `cli` (43), `cyber-server` `http` (62) and `cyber-app` `workflows`. Native and windows-gnu clippy with `-D warnings` and `openspec validate --all --strict` (63 items) also pass. An unrelated `io-extras` `windows-sys` re-resolution in `Cargo.lock` was reverted, so the lockfile only adds the new crate and the rquickjs packages.

Native Windows runs of the new tests are pending CI.

## M2.1 Workflow runtime, increment 2 of 4: runtime core (2026-10-16)

Increment 2 delivers what the brief named "runtime core": real agents, the global limit, tiers, and runs started over HTTP. It comes ahead of resume, which the original task plan had placed in this increment. `tasks.md` section 6 lists what was delivered; tasks 3.2–3.4, 4.3, 4.7 and 5.1 are annotated as partly done.

### What runs now

- **HTTP.** `POST /api/v1/workflows/runs` takes `{session_id, script | name, args}` and answers `201` with the run. `GET /api/v1/workflows/runs[?session_id=]` lists runs, and `GET /api/v1/workflows/runs/{id}` shows a run with its settled agents. Every run has an owner Session, whose Location, Mode and rules apply to the run. Runs execute in the server, independent of the client. Other workflow paths still answer `503` with `service: workflow`.
- **Engine thread.** Each run drives the QuickJS engine on its own thread. Agent calls cross to the tokio runtime, and dropping a pending call cancels its child.
- **Agents.** `ToolHost::spawn_workflow_agent` is unsupported by default. `BuiltinHost` implements it next to `subtask_request` by synthesizing an `agent` invocation with `operation_key = <run_id>:<call_key>`, and `AppHost` forwards it (the forwarding guard test covers this).
  - `agent.rs` now spawns every child through one path keyed by `SpawnSource` (`Model`, `User`, `Workflow`).
  - Each child Session carries `workflow_run_id`, uses its `agt_` ID as subagent name, and runs under the owner's Mode as a ceiling. Deny rules apply.
  - The result is `{ text, session_id, cost, tokens }`, or the validated object for a `schema` after a single result retry.
  - Failures are typed: `AgentFailed`, `SchemaMismatch`, `PermissionDenied` and `ModelUnavailable`.
  - `goal`, `tools`, `budget`, `isolation: "worktree"`, `checkpoint()` and `ask()` reject explicitly as not available.
- **Concurrency.** One FIFO semaphore per runtime, `min(workflows.max_concurrent, cores)`, is set by `App::build`. The permit travels into `ChildGuard`, which releases it only after the child settles. `agents.max_concurrent` does not apply to workflow agents.
- **Tiers.** `fast`, `smart`, `verify` and custom tiers resolve per Location. `tier_role_ref` falls back to the default model, so `verify` works without an advisor. An unknown tier is `ModelUnavailable`.
- **Durability.** The `run_` aggregate (migration `20261016000000_workflow_runs`) has the events `workflow.run.{started,completed,failed}.1`, `workflow.agent.settled.1`, `workflow.phase.1` and `workflow.log.1`, projected into `workflow_run` and `workflow_agent`.
  - The settled event is appended before the script sees the result.
  - A failed append ends the run through `WorkflowHost::fatal`, and failed phase or log records fail it directly.
  - Call keys are `sha256(prompt + canonical options)` plus an ordinal.
- **Model.** `SessionInfo.workflow_run_id` is new, so `sdk/openapi.json` and the generated TS types were regenerated. The workflow routes are still outside OpenAPI (increment 4).

### Tests

- **`cyber-workflow` engine:** 3 new tests (19 engine tests in total):
  - `parallel_group_concurrency_bounds_calls_in_flight`
  - `a_fatal_host_failure_ends_the_run_before_the_script_sees_the_result`
  - `a_failed_phase_or_log_record_fails_the_run`

  Phase appending and log truncation are covered by the existing tests.
- **`cyber-llm` roles:** `tier_roles_fall_back_to_the_default_model` and `roles_parse_from_their_keys`.
- **`cyber-app` services:** tier resolution through `model_roles` (with `verify` falling back to the default), and limits from the `workflows` section.
- **`cyber-server` `workflow_runs`** (fake agent host, 6 tests):
  - the settled record is stored before a dependent call starts (ordering assertion), with the exact event sequence
  - `agt_` IDs, call keys, owner Mode and phases
  - ordinals and failed records
  - the global limit of 2 is never exceeded across two runs, and is FIFO (`a3` starts before any `b*`)
  - tiers and `ModelUnavailable`
  - unavailable options reject before any agent starts
  - a host without workflow agents fails closed, and a missing owner is refused

  Writing this suite exposed that an `async move` block captures only the fields it uses. The fake host must bind the permit explicitly; `BuiltinHost` moves the whole request.
- **`cyber-tools` `workflow_agents`** (`BuiltinHost` with scripted models, 4 tests):
  - children carry `workflow_run_id` and the `agt_` name and inherit the `plan` Mode
  - a missing result is retried once and then reported as `SchemaMismatch`
  - a provider error is reported as `AgentFailed` and a deny rule as `PermissionDenied`
  - with `agents.max_concurrent: 1`, three workflow agents still stream concurrently (a rendezvous adapter)
- **`cyber-app` `workflows`** (production App, AppHost and AppServices over authenticated TCP, with a local OpenAI-compatible scripted model, 3 tests):
  - An inline `parallel([agent(a), agent(b, {schema})])` script, started with `POST /workflows/runs`, completes when polled.
  - It returns the text result and the schema-validated object.
  - Both child Sessions report `workflow_run_id`, `parent_id` and the `agt_` name over `GET /sessions/{id}`.
  - The model saw exactly one plain request and one request offering `return_result`.
  - Script failures with a location, catchable `ModelUnavailable`, and `400` (diagnostic), `404` and `401` errors are covered, and unimplemented paths still answer `503`.
- **CI:** the native Windows step now also runs `cyber-server --test workflow_runs` and `cyber-tools --test workflow_agents`.

### Not done in this increment

- Resume and replay, pause and stop, recovery, and shutdown marking runs `paused`.
- `workflow.call.recorded.1`, checkpoints and `ask()`.
- Run budgets, `max_agents` and reserved enforcement (4.16).
- The `workflow.run` permission, and auto-rejecting agent asks in non-interactive runs (4.12). Today an agent's permission ask waits for an interactive client like any child's.
- `CYBER_WORKFLOW_RUN_ID` in tool processes.
- Write isolation, read-only `tools`, warm-up, and result handback to the owner Session.
- OpenAPI and the SDK run handle, the CLI run commands, and the TUI monitor.


### Verification (macOS, 2026-10-16)

- **Pass:**
  - `cargo fmt --check`.
  - Native clippy with `-D warnings` (all targets) for `cyber-workflow`, `cyber-server`, `cyber-tools`, `cyber-app`, `cyber-llm`, `cyber-store`, `cyber-core` and `cyber-cli`.
  - `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR, all targets) for `cyber-workflow`, `cyber-server`, `cyber-tools`, `cyber-app` and `cyber-llm`.
  - Full test suites of `cyber-server`, `cyber-store`, `cyber-workflow`, `cyber-llm`, `cyber-tools`, `cyber-app` and `cyber-cli`.
  - `openapi_document_is_current` and `generate_sdk.py --check`.
  - `openspec validate --all --strict` (63 items).
- **Stack overflow found and fixed.** `cyber-tools` `bundled_skills::bundled_batch_load_can_delegate_five_real_isolated_coding_tasks` overflowed its stack after the shared spawn path was introduced, because the larger spawn future was nested inline. `run_with_skill` and `run_workflow` now box it, and the existing test is the regression guard.
- **Test flake found and fixed.** The FIFO test was flaky at first: two permits were released together, so the order in which agents recorded their start was a race. Staggered releases make the grant order observable, and the test then passed 8 of 8 repeated runs.

## M2.1 Workflow runtime, increment 3 of 4: durable replay and resume, lifecycle, run budgets (2026-10-16)

### What runs now

- **Replay** (`runtime/workflow_replay.rs`).
  - Every host call gets an issue sequence number per revision. The call key is `sha256(prompt + canonical options)` for an agent and the name for a checkpoint, plus an ordinal.
  - `workflow_call` (migration `20261017000000_workflow_budget`) stores each call's key, its agent, its outcome and the order the script received it.
  - A relaunch of the same revision delivers the recorded results in that order without calling the host. Because the script is deterministic, it reaches the same state.
  - A call issued during that prefix with a different key, or a recorded result whose call is never issued, fails the run with `WorkflowReplayMismatch`. The engine only polls host futures while the script is idle, so the check is exact.
  - The prefix ends at the first recorded failure, so a resume retries a failed agent.
  - After the prefix, and in a new revision, calls are matched by key: completed results and checkpoint values are reused, an unsettled agent resumes its existing child Session, and other calls run anew.
  - Logs and phases of the replayed part are not recorded again.
- **Revisions.** Each revision stores its script source, its digest and a configuration digest (limits and tiers) in `workflow_run_revision`. Inline scripts are relaunched from the stored source, and saved Workflows are read again. A changed digest without `revision: true` is refused with `409 WorkflowReplayMismatch`.
- **Resume of unsettled agents.** The request carries `resume: true`, so `BuiltinHost` passes `resume: <agt_ID>` through the shared `agent.rs` path (`resolve_subagent`, `claim_child_execution`, `resume_child`) with a continuation prompt. A child whose admission a stop closed is reopened first: `reopen_subtree` with the stop receipt. The child's next Drain runs `recover`, which reconciles interrupted tool calls through `ToolHost::reconcile` before any further mutation.
- **Lifecycle** (`runtime/workflow_control.rs`).
  - The events `workflow.run.{paused,resumed,revised,stopped}.1` join `started`, `completed` and `failed`, together with `workflow.agent.started.1` and `workflow.call.recorded.1`.
  - A process-local `Control` per attempt (like `jobs.rs` `Control`) holds the signal (running, pausing, stopping, closing) and counts leases: the engine thread and every agent task.
  - Pause is recorded at once; in-flight agents finish and are stored, and the next `agent()` ends the attempt once nothing is in flight.
  - Stop cancels in-flight agents and calls `stop_subtree` for each unsettled child, keeping the stop receipts. A cancelled agent is never recorded as settled, so a resume continues it.
  - Resume is refused with `Run has agents still exiting; retry shortly` while a lease is held or an agent child's Drain is running.
- **Server shutdown and recovery.** `Runtime::shutdown` durably pauses running runs (agents `interrupted`) before admission closes, then waits (bounded) for engines and agent tasks after the Drains settle, before `tools.shutdown`. `App::build` calls `recover_workflow_runs` next to `recover_jobs`, and it pauses runs that a crashed process left `running`. The native Windows service path runs the same `Runtime::shutdown` and `App::build`.
- **Run budgets.**
  - `RunBudget` (`cyber-core`) layers `budgets.run`, `meta.budget` and the request's `budget`, with defaults `max_agents` 200 and `max_wall_seconds` 14400. `reserved` is refused.
  - `workflow_run_budget` joins `runtime/budget.rs` scopes through the `workflow_run_id` of the Session or any ancestor (now a `session` column, backfilled from `session.created.1`). The existing check before every child provider Turn therefore enforces the run's cost, token, Turn and wall limits. Wall time excludes paused segments.
  - Before a new agent starts, `check_run` applies the same limits plus `max_agents` and records `budget.exceeded.1` on the run (signals in `workflow_run_budget_signal`).
  - The call rejects with `BudgetExceeded: <limit>`, and the run ends `budget_exceeded`, keeping a partial result. A child refused by the run budget also rejects with `BudgetExceeded`.
  - `ctx.budget` is a live getter that the engine refreshes before every JavaScript slice: `{ spent_tokens, spent_cost_usd, agents_started, remaining }`.
- **Usage.** `GET /api/v1/usage?scope=run&id=` returns the run's total and its rollups per agent, model and tier (`RunUsageReport`). `UsageReport.scope` and `RunUsageReport.scope` are literal discriminants of the `ScopeUsage` union (OpenAPI and TS SDK regenerated).
- **HTTP.** `POST /workflows/runs/{id}/pause|resume|stop` and `GET /workflows/runs/{id}/logs`. Resume takes `{revision?, budget?}`, and start takes `budget`. Lifecycle conflicts answer `409`.

### Tests

- **`cyber-workflow` engine:** `ctx_budget_is_a_live_frozen_host_snapshot`.
- **`cyber-core`:** `run_budgets_layer_config_meta_and_invocation_with_defaults`.
- **`cyber-server` replay units (5):**
  - the prefix follows delivery order and stops at the first failure
  - recorded results are delivered in recorded order
  - a changed or reordered call is a mismatch
  - a recorded result whose call is never issued is a mismatch
  - notes of the repeated part are not recorded again
- **`cyber-server` `workflow_runs` (8 new, fake agent host plus real child Sessions on the scripted model):**
  - pause lets the in-flight agent finish and starts none, and resume replays `a`, the checkpoint and `b` from the store, so only `c` reaches the host and logs and phases are not repeated
  - a changed script without a revision is refused, while a new revision reuses `a` by key, returns the recorded checkpoint value and runs only the edited call
  - stop cancels agents and records no settlement; resume is refused while a stubborn agent still holds its task, then succeeds (eventual state) and resumes both agents under their original IDs
  - a settlement that cannot be recorded fails the run closed
  - `max_agents: 1` rejects the second agent, gives `budget_exceeded` and records `budget.exceeded.1` on the run
  - `ctx.budget` reflects real child spend, and wall time is enforced
  - a run cost cap refuses a real child's second provider Turn, and `run_usage` reports it
  - shutdown and startup recovery leave the run paused, and a new runtime over the same store resumes the interrupted agent
- **`cyber-app` `workflows` (production App over authenticated TCP, 4 new):**
  - A three-agent sequential run, whose scripted model holds agent 3's request, is restarted on the same database file. It comes back `paused` with agent 3 `interrupted`. `POST …/resume` completes it: the model got no new request for agents 1 and 2, agent 3 continued its existing child Session (the same child set), and `…/logs` holds the single log entry.
  - `POST …/stop` over HTTP stops the real child's subtree. Resume answers `409` until the agent exits, then reopens the child's admission (stop receipt before `session.admission.reopened.1`) and completes.
  - `budget: {max_agents: 1}` over HTTP ends `budget_exceeded`, and a `reserved` budget answers `400`.
  - A mixed `fast`/`verify` run reports per-model cost through `GET /usage?scope=run` (the verify model priced 10× costs 5× two fast agents) and per-tier rollups.
  - `scripted_model.rs` now serves each connection on its own thread and can hold a request.
- **`cyber-app` `windows_service`:** a native service stop pauses a running run whose agent waits for a slot, and a rebuilt App keeps it paused. Only type-checked on macOS (windows-gnu clippy); the native Windows CI step runs it.
- **`cyber-server` `http`:** `scope=run` for a missing run answers `404`, and other unavailable scopes still answer `503`.
- **TS SDK:** `usage.get narrows a Workflow Run rollup by its scope`.

### Not done in this increment

- `ask()` and `waiting_input` (reserved), read-only `tools`, warm-up, worktree isolation, reserved budget enforcement (4.16), the `workflow.run` permission, and `CYBER_WORKFLOW_RUN_ID`.
- Workflow routes in OpenAPI, the SDK run handle, live run events on the event stream, CLI run commands and the TUI monitor (increment 4).
- `ctx.budget` reads live spend. A script that branches on it can diverge on replay, which fails closed with `WorkflowReplayMismatch`.

### Verification (macOS, 2026-10-16)

- **Pass:**
  - `cargo fmt --check`.
  - Native clippy with `-D warnings` (all targets) for `cyber-server`, `cyber-app`, `cyber-workflow`, `cyber-core`, `cyber-tools`, `cyber-store` and `cyber-cli`.
  - `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR, all targets, including `windows_service.rs`) for `cyber-server`, `cyber-app`, `cyber-workflow` and `cyber-tools`.
  - Full test suites of `cyber-server`, `cyber-store`, `cyber-workflow`, `cyber-app`, `cyber-tools` and `cyber-core`, plus `cyber-cli --test cli workflow`.
  - TS SDK `npm test` and `typecheck`, `openapi_document_is_current` and `generate_sdk.py --check`.
- **Flake (not this change).** `cyber-app` `mcp_stdio::local_call_timeout_sends_cancelled` failed once under full-suite load ("was never published"). It passed alone twice and in the next full run.

## M2.1 Workflow runtime, increment 4 of 4: surfaces and safety (2026-10-16)

### What landed

- **Permissions.** `permissions::defaults` has an explicit `workflow.run * ask` rule with source `default`, which `evaluate` reports. `auto` allows only that default rule without the classifier; `bypass` allows; a configured rule (for example a deny on one workflow name) wins in every Mode. The `workflow` tool's approval carries `name`, `description`, `phases`, `budget`, `origin` and `digest`.
- **Unattended agents.** A run records `interactive`. Runs started over the API or the CLI are not interactive: `requests.rs` resolves any ask of a Session that belongs to such a run (its own `workflow_run_id` or an ancestor's) as `Unattended` at once, and the tool reports `No interactive approver for workflow run`. A run that cannot be read counts as not interactive.
- **Write isolation.** `ToolHost::workflow_agent_writes` (BuiltinHost: git checkout plus an edit tool in the profile's tool set; AppHost forwards it, guarded by `app_host_forwards_every_tool_host_method`). Agent calls without an `isolation` choice are counted when issued, so a `parallel()` fan-out is counted before its first agent starts. A new agent gets `RunWorktree` (`run-<ulid>-<index>` on `cyber/run-<ulid>/<index>`, index = start ordinal) when another writer is in flight; `isolation: "worktree"` always isolates and `"none"` opts out. `create_child_worktree` takes the run naming, authorizes deny-only and keeps the checkout (`Cleanup::Keep`). Results and `workflow_agent` records carry `branch`.
- **Run results.** `workflow_results.rs`: JSON up to 1 MiB stays inline, larger values go to `<state>/workflows/<run_id>.result.json` with a 16 KiB preview (a missing state directory fails the run closed). Every ended run (completed, failed, budget-exceeded, stopped) fires `WorkflowRunEnd`, then admits one queued summary `msg_workflow_<run_id>` with source `workflow` (waking the owner only for tool-started runs) and records `workflow.run.notified.1`. Startup recovery admits unrecorded summaries again; an existing message with that ID counts as admitted.
- **`workflow` tool.** `tools/workflow.rs` resolves the module and budget through the shared `runtime::workflow_start` rules (also used by HTTP), asks `workflow.run`, starts the run as `RunStarter::Session` and returns the handle. Inline scripts are kept at `<state>/workflows/<run_id>.js` (`script_file`) by the runtime, for HTTP runs too. Workflow agents cannot start runs. `action_of("workflow")` is `workflow.run`, so a full deny hides the tool.
- **CLI.** `cyber workflows run/runs/show/pause/resume/stop/logs` use `cyber-client` against the shared server. `run` creates the owner Session (Mode `--mode`, else `default`) and sends a saved name or a file's source; `--max-cost/--max-turns/--max-tokens/--timeout/--max-agents` become budget fields. `--wait` polls and exits 0/1/4/130 (Ctrl-C stops the run). `--runner` exits 2 as unavailable before any server starts.
- **API and SDK.** Seven `v1.workflow.*` operations in `openapi.rs` with JsonSchema on the run types (`schemars` added to `cyber-workflow` for `Diagnostic`); `sdk/openapi.json` and the TS sources regenerated. `sdk/typescript/src/workflows.ts` adds `client.workflows.run()` with `events()`, `status()`, `pause()`, `resume()`, `cancel()` and `result()` (rejecting with `WorkflowRunError`), plus `examples/run-workflow.ts`.
- **Live events.** Run events are published on the bus through `RunLog`; `event_location.rs` maps them to the directory in `workflow.run.started.1`, so they stream on `GET /api/v1/event` for that Location.
- **Hooks.** `HookIdentity.workflow_run_id` (reserved, `run_` prefix) comes from the Session envelope for workflow agents, and from the run for `WorkflowRunStart`/`WorkflowRunEnd` (runtime lifecycle events on the owner) and for a workflow agent's SubagentStart/SubagentStop. **Bug fixed:** SubagentStart used to pass `workflow_run_id` as a payload field, which the envelope now reserves; it moved into the envelope (regression: `cyber-app` `server::application_emits_runtime_lifecycle_hooks_with_session_identity` and the workflow hooks test).

### Tests (production path first)

- **`cyber-cli` `cli::workflows_run_against_the_shared_server_and_wait_maps_exit_codes`:** the real `cyber` binary starts the registered shared server with a scripted local OpenAI-compatible model. `workflows run x --wait` exits 0 with progress and the result, a throwing `boom.js` exits 1, `--max-agents 0` exits 4, `--args` that is not an object exits 2, `runs`/`show`/`logs` print the runs, resuming a completed run exits 1, and `--runner` exits 2.
- **`cyber-app` `workflows` (production App, 4 new):** the model's `workflow` tool in `default` Mode asks with name, phases and budget before any run exists, then the run completes, its script is kept under `state/workflows`, and the owner gets exactly one `msg_workflow_<run_id>` entry and answers it; a deny rule refuses the tool in `bypass` and keeps nothing; an API run's agent `bash` ask is rejected with the exact message while the instance SSE stream delivers the run's `workflow.*` events with the run's Location; (unix) `WorkflowRunStart`, `WorkflowRunEnd`, the agent's SubagentStart and its `PreToolUse` all carry `workflow_run_id`.
- **`cyber-tools` `workflow_agents` (2 new):** with `Fixture::native_git()` (explicit full access on Windows only), two parallel writers get distinct worktrees on `cyber/run-<id>/1` and `/2` reported in their results, a lone writer and `isolation: "none"` stay in the checkout, and the worktrees are kept; the `workflow` tool starts an owned run (golden `goldens/workflow.txt`), is denied by a name rule in `bypass`, and is refused inside a workflow agent.
- **`cyber-server`:** `workflow_runs` (3 new: isolation decisions and numbering with a fake writer host; large result to a file with the summary admitted once and again once after a simulated restart; API runs are not interactive), `workflow_results` units (inline vs file, handback text), `workflow_start` units (moved from `cyber-app`, plus budget layering).
- **Units:** permission engine `workflow_run_asks_by_default_with_a_visible_source_and_is_allowed_in_auto_and_bypass`; `cyber-core` `hook_envelopes::workflow_agent_envelopes_carry_the_run_and_payloads_cannot_forge_it`; CLI exit-code mapping.
- **TS SDK:** `test/workflows.test.ts` (3) against the generated types; `npm test` (86), `typecheck` and `build` pass.
- **Windows:** the native job's workflow step adds the server lib, permission, envelope and CLI unit tests to the existing workflow suites.

### Verification (macOS, 2026-10-16)

- `cargo fmt --check`; native clippy `-D warnings --all-targets` for `cyber-core`, `cyber-server`, `cyber-tools`, `cyber-app`, `cyber-cli` and `cyber-workflow`; `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR) for the same crates.
- Full test suites of those crates; the only failures in the first full run were the SubagentStart bug above and suites that need the `cyber-sandbox-exec` helper, which `cargo clean` had removed (they pass after `cargo build -p cyber-sandbox --bins`).
- `openapi_document_is_current`, `generate_sdk.py --check`, `cargo metadata --locked` and `openspec validate --all --strict` (64 items).

### Still open after M2.1

`ask()`/`waiting_input`, direct read-only `tools`, prompt-cache warm-up, `CYBER_WORKFLOW_RUN_ID`, compaction reporting, the TUI monitor and Save action, `exec --workflow`, Job integration, saved-Workflow listing over HTTP and capability discovery, reserved budgets (4.16), M2.2 bundled Workflows, P3 runners.

## M2.1 Workflow runtime: review fixes

Fourteen review findings were checked; thirteen were real (two of them, the HTTP and CLI `workflow.run` check, were one bug). Each fix has a regression test that fails without it.

- **Replay of parallel batches (blocker).** `RunHost::agent`/`checkpoint` now take their sequence number when the engine dispatches the call, not when the future is first polled, so every call of a `Promise.all` batch is issued before any is served. Before, a batch whose results arrived out of order failed its resume with `WorkflowReplayMismatch`. Test: `workflow_runs::a_parallel_batch_settled_out_of_order_resumes_through_replay`; unit `workflow_replay::a_batch_issued_together_replays_in_recorded_order_whatever_is_polled_first`.
- **Rejected calls are sequenced.** Option errors, unknown tiers, invalid payloads (new `WorkflowHost::rejected`) and `ask()` are issued and recorded as `workflow.call.recorded.1` errors, so a script that catches them replays without a false mismatch. Test: `workflow_runs::a_caught_rejection_beside_a_parallel_agent_replays_without_a_mismatch`.
- **Pause stops queued agents.** The global FIFO permit is taken before `workflow.agent.started.1`; a call that waited for it while the run paused or stopped starts nothing and stays unrecorded for the resume. Test: `workflow_runs::agents_queued_behind_the_global_limit_do_not_start_after_a_pause`.
- **Lifecycle race.** `finish()` records the script's end under `workflow_lifecycle` and only while the run is still `running` or `paused`, so a stop never coexists with `completed` and a pause never overwrites a completed run's result. Test: `workflow_runs::pause_and_stop_never_overwrite_or_duplicate_the_scripts_own_end` (20 rounds each).
- **`workflow.run` over HTTP and the CLI.** `Services::authorize_workflow_run` (AppServices: `BuiltinHost::workflow_run_denial` over the owner Session's rules) refuses a deny for the bare name or the qualified resource with `403 PermissionDenied`, on start and on resume; `ask` counts as approved for an authenticated client. Tests: `cyber-app` `workflows::http_starts_and_resumes_honour_workflow_run_deny_rules`, `cyber-cli` `cli::a_workflow_run_deny_rule_refuses_cyber_workflows_run`.
- **Origin-qualified resource.** `permissions::workflow_resource`: `<name>` (global saved), `project:<name>`, `inline:<name>@<digest>`; the tool checks the bare-name deny first (`Ctx::refuse_denied`). An allow or "always" approval no longer covers another script declaring the same name. Tests: unit `a_workflow_allow_rule_or_approval_covers_only_its_origin`; the `cyber-app` tool test now expects `inline:tooled@sha256:…`.
- **`ctx.budget` replay.** The snapshot is recorded at the revision start (`run.started`/`run.revised`) and with each delivered result (`budget_snapshot`), and replayed as recorded; it no longer reads live spend between results. Test: `workflow_runs::ctx_budget_replays_as_recorded`.
- **Mode ceiling on resume.** `launch` uses the stricter of `run.mode` and the owner's current Mode. Tests: `workflow_runs::a_resume_keeps_the_mode_the_run_started_with_as_a_ceiling`, `cyber-app` `workflows::a_resumed_run_keeps_its_starting_mode_as_the_agents_ceiling`.
- **Summary after resume.** A resume clears `notified`, `result_file` and `result_preview`; the summary of the attempt after the `n`th resume is `msg_workflow_<run_id>_<n>`. Tests: `workflow_runs::a_resumed_run_hands_back_again_and_drops_an_earlier_result_file`, `cyber-app` `workflows::a_tool_started_run_that_is_resumed_hands_back_each_end_and_drops_a_stale_result_file`.
- **`meta` nesting (blocker).** The literal reader stops at 64 levels with `ParseError`, so a deep literal can no longer abort the server through the tool, HTTP or discovery. Tests: `meta::deep_nesting_fails_without_overflowing_the_stack`, `discovery::deeply_nested_meta_is_an_issue_not_a_crash`, `engine::deeply_nested_scripts_fail_without_crashing`.
- **Error description charged (blocker).** `run_jobs` and `outcome` describe errors inside the charged slice, so `throw { toString() { for (;;) {} } }` ends with `Timeout`. Test: `engine::describing_a_hostile_error_is_charged_to_the_script`.
- **Bounded native bridge.** `Bridge` enforces 10,000 pending calls, payload and phase bytes within `max_memory_mb`, and the 1 MiB log cap as the script calls it; a refused call throws `ExecutionFailure` synchronously. Tests: unit `the_bridge_bounds_queued_logs_calls_and_bytes`, `engine::synchronous_call_loops_are_bounded_by_the_bridge`.
- **Ctrl-C in `run --wait`.** One `ctrl_c` listener covers the poll and the sleep. Test (unix): `cli::ctrl_c_between_polls_stops_a_waited_run` (failed with the old loop).

Spec delta and `local-core-guide.md` updated (Permissions for runs, Run results, Run lifecycle states, Run budgets, Replay of workflow host calls, module format and sandbox). `openspec validate implement-m2-1-workflow-runtime --strict` passes.

## M2.1 Workflow runtime: review fixes, round 2

Seven findings were checked; all seven were real. Each fix has a regression test, and the replay and slot tests were confirmed to fail with the fix reverted.

- **Failed resumed agents stayed "unsettled".** `Replay::load` cleared a call key's unsettled `agt_` only on a completed settlement, so after revision 2 resumed revision 1's interrupted agent and it failed, every relaunch resumed the failed child again (with the interrupted-attempt prompt) and skipped `charge_agent`. `index_call` now clears it on any settlement. Test: `workflow_runs::an_agent_that_failed_when_resumed_is_retried_as_a_new_agent`.
- **Pause while queued for a global slot.** `RunHost::slot` raced the FIFO acquire only against cancellation, so a run paused while waiting kept its place in the queue, held its engine lease and refused resume with `AGENTS_EXITING` until another run freed a slot. The acquire now also races `signalled()` (signal leaves `Running`), drops the queued acquire and halts through `admit()`. Test: `workflow_runs::a_run_paused_while_queued_for_a_slot_leaves_the_queue_at_once`.
- **Budgets could be widened by the model or a script.** `RunBudget::resolve` let `meta.budget` and the invocation replace fields, and `null` removed a cap. Configuration and the defaults are now a ceiling (`tighten`: lower only, `null` for a set limit refused); `RunBudget::overridden` keeps the explicit raise for the authenticated client (HTTP start via `workflow_start::client_budget`, resume). Tests: unit `budget::run_budgets_layer_config_meta_and_invocation_with_defaults`, `workflow_start::run_budgets_layer_configuration_meta_and_invocation`, `cyber-app` `workflows::the_workflow_tool_cannot_lift_configured_run_budgets`.
- **Unbounded host-call records and no wall clock without agents.** `RunHost::append` charges every record against `MAX_RUN_RECORDS` (100,000) and `MAX_RUN_RECORD_BYTES` (256 MiB), seeded from the run's stored events; a checkpoint value over 1 MiB fails the run closed; past `max_wall_seconds` the first non-agent call rejects with `BudgetExceeded` and the next fails the run closed (status `budget_exceeded`). Tests: unit `workflow_host::records_are_bounded_by_count_and_bytes`, `workflow_runs::a_checkpoint_loop_ends_at_the_wall_budget_and_large_checkpoints_fail_closed`.
- **Unbounded phases.** The bridge charges each queued entry its text plus `size_of::<Op>()`, does not queue a repeat of the last queued phase and refuses titles over 200 characters; the engine skips a phase equal to the current one and fails past 256 distinct titles (`MAX_PHASES`), as does `RunHost::phase` across revisions; `workflow.phase.1` carries `phases` only when a title is added. Tests: unit `tiny_phases_are_charged_per_entry_and_repeats_are_not_queued`, `engine::phase_loops_are_bounded_and_repeats_do_not_reach_the_host`, `workflow_runs::phase_loops_record_changes_only_and_stop_at_the_title_cap`.
- **Nested runs over the API.** `Runtime::start_workflow_run` refuses an owner that is, or descends from, a workflow agent (`409`, `WORKFLOW_AGENT_STARTS_RUN`), failing closed when the ancestry cannot be read (`workflow_run_of`); the tool uses the same fail-closed lookup. Tests: `workflow_runs::a_workflow_agent_session_cannot_own_a_run`, `cyber-app` `workflows::a_workflow_agent_session_cannot_own_a_run_over_http`.
- **Edited `script_file` ignored on resume.** HTTP resume of an inline run reads `script_file` (stored source only when the file is gone), then re-runs `authorize_workflow_run` on it; an edit without `revision: true` answers `409 WorkflowReplayMismatch`. Test: `cyber-app` `workflows::an_edited_script_file_relaunches_as_a_new_revision`.

Spec delta (sandbox, Phases, logs and checkpoints, Permissions for runs, Model-authored workflows, Run lifecycle states, Run budgets, Replay) and `local-core-guide.md` updated.

## M2.1 Workflow runtime: end-to-end acceptance with the real binary (2026-10-16)

The debug `cyber` binary was exercised in a temporary git project with its own `HOME` and `CYBER_HOME`, against a scripted local OpenAI-compatible stub (Python, `127.0.0.1` only, logging each request with start and end times). No live model was used. `workflows.max_concurrent` was 2.

| # | Scenario | Command | Observed | Result |
|---|---|---|---|---|
| 1 | Saved fan-out | `cyber workflows list`; `cyber workflows run fanout --wait` (`phase`, `parallel` of 3 schema `agent()` calls, `pipeline`, `log`) | Listed as `fanout [project]`. Exit 0, result `{"total":140,"ranked":[…]}` built from the three schema results. Stub times: two requests overlapped and the third started only after both ended (cap 2). Each agent is a child Session (`v1.session.children`) with `workflow_run_id`, its `agt_` name and its own `cyber/run-<id>/<n>` worktree. `logs` prints `analyzed 3` and the ranking. | pass |
| 2 | Inline run over HTTP | `cyber api POST /api/v1/workflows/runs` with `session_id` and `script`, then `GET /api/v1/workflows/runs/{id}` | Answer `running`; polls show agents `running`, then `completed` with `{"joined":"answer for one \| answer for two"}`. | pass |
| 3 | Determinism guards | `cyber workflows run <file>.js --wait` with `Date.now()`, `Math.random()`, `import fs`, `require('fs')`, `fetch`, `process` | `ExecutionFailure: non-deterministic API; pass values through args` (exit 1) for the clock and random calls; `UnsupportedSyntax: import of "fs" …` / `require of "fs" …` with `line:col` (exit 2, nothing started); `fetch is not defined` and `process is not defined` (exit 1). The stub's request count was unchanged (5 before, 5 after), so no agent ran. | pass |
| 4 | Resume after a crash | `kill -9` of the server while agent 2 of 2 was in flight, `cyber service start`, `cyber workflows resume <id>` | Before the fix the resumed agent failed: `Subagent execution ownership is held or requires recovery`. After it: the run is `paused` with agent 2 `interrupted`, the resume completes, agent 1 is replayed with no new request for its prompt, and agent 2 continues its existing child ("Your previous attempt … was interrupted"), 1 new request in total. A graceful `cyber service stop` behaved the same before and after. | pass (after fix) |
| 5 | Budget | `cyber workflows run seq --max-tokens 50 --wait`; same for `fanout` | `budget_exceeded`, exit 4, `BudgetExceeded: max_tokens`; the sequential run made 1 model request before the next `agent()` was rejected. In the parallel run the agents in flight are now listed `cancelled` (they stayed `running` before the fix). | pass (after fixes) |
| 6 | Stop | `cyber workflows stop <id>` on a run of held agents | `stopped`; the agents are `cancelled`, their child Sessions end "Provider turn interrupted", and the third agent (queued behind the cap) never starts. | pass |
| 7 | Permissions and Mode ceiling | deny rule `workflow.run: {fanout: deny}`; `cyber --mode plan/default/accept-edits workflows run writer/basher --wait`; `cyber exec "…"` with a model that calls the `workflow` tool | The CLI and HTTP start answer `Permission denied: workflow.run is denied for fanout` (exit 1, no model request) while `seq` still runs. In `plan` the agent's `write` gets "Plan mode is read-only"; in `default`, `write` and `bash` get "No interactive approver for workflow run"; in `accept-edits` `write` succeeds and `bash` is still rejected. Child Sessions carry the run's Mode. `cyber exec` (dont-ask) denies the `workflow` tool ("Not pre-approved"), and `--mode auto` starts the run. | pass |

### Bugs found and fixed (each with a regression test that fails without the fix)

- **Crash left workflow agents unresumable.** A server killed while it owned a child left its `child.execution.changed.1` claim `held` forever, so resuming the run failed the agent. Claims now record the registration ID of the server that holds `server.lock` (`Runtime::adopt_server_lock`, called by `run_server_until` once the lock is acquired). The next lock holder releases claims an earlier server left held; claims without a holder (runtimes that never took the lock) stay held. Tests: `cyber-server` `child_ownership::the_next_server_lock_holder_releases_only_claims_of_an_exited_server`; `cyber-cli` `cli::a_run_resumes_after_the_server_is_killed_mid_agent` (unix, real binary, `kill -KILL`).
- **Agents stayed `running` after their run ended.** When a script ended (completed, or failed, for example on a `BudgetExceeded` rejection inside `parallel`) with agents in flight, those agents were cancelled but their records stayed `running`. `workflow.run.completed.1` and `workflow.run.failed.1` now mark them `cancelled`, as stop does. Test: `workflow_runs::agents_in_flight_when_the_script_ends_are_recorded_cancelled`.
- **`run --wait` progress.** Agents were tracked by count, so an agent settling before an earlier-listed one was printed twice and the other never; a phase change was printed before the agents of the previous phase that settled in the same poll. Progress is now tracked by agent ID and ordered by phase. Test: `commands::workflows::tests::progress_prints_each_settled_agent_once_in_phase_order`.
- **Doubled error kind.** `error: BudgetExceeded: BudgetExceeded: max_tokens` is now `error: BudgetExceeded: max_tokens`. Test: `commands::workflows::tests::run_errors_name_their_kind_once`.

### Noted, not changed

- `cyber exec --mode default` waits for an approval reply when the model needs one (here the `workflow` tool or `bash`), which looks hung in text mode. This is exec-mode behaviour that is not specific to workflows. Exec's default `dont-ask` and `auto` behave as specified.
- `cyber sessions` is not shipped; child Sessions were inspected through `cyber api v1.session.children`.

### Verification (macOS, 2026-10-16)

- `cargo fmt --check`; native clippy `-D warnings --all-targets` and `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR) for `cyber-server`, `cyber-app` and `cyber-cli`.
- Full test suites of `cyber-server` (40 test binaries), `cyber-app` and `cyber-cli` (396 tests) pass.
- `openspec validate --all --strict` (64 items).

### Whole working tree validation (macOS, 2026-10-16)

- `cargo test --workspace --no-fail-fast`: 2755 passed, 0 failed, 4 ignored.
- `cargo clippy --workspace --all-targets -- -D warnings` and `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR) for `cyber-app`, `cyber-cli`, `cyber-core`, `cyber-server`, `cyber-store`, `cyber-tools`, `cyber-workflow` and `cyber-llm`: clean.
- `cargo fmt --all -- --check`, `generate_sdk.py --check`, SDK `npm run typecheck` and `npm test` (86 tests), `openspec validate --all --strict` (64 items), `spec_lint.py` (0 errors, 21 warnings, the baseline) and `spec_inventory.py` (no ROADMAP drift).

### Not part of M2.1

- `CYBER.md` instruction files ship in the separate change `add-cyber-md` (loading through the baseline, nested on-read discovery and `cyber doctor`, from one ordered list of file names in `crates/cyber-server/src/runtime/instructions.rs`; evidence in [P1 status](p1-status.md#2026-10-16-cybermd-project-instructions-add-cyber-md)).
