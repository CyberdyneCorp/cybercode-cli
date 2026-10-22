## Context

Workflows are untrusted code. A script can come from a repository, from the model as an inline script, or later from a plugin. The script orchestrates agents that have tool access. The runtime must therefore fail closed, and it must preserve the existing trust and digest checks, sandboxing, permissions, Mode ceilings, credential redaction, and durable receipts and ownership. Agents are ordinary child Sessions, so every tool call still goes through the permission engine and the sandbox. The script itself gets no capability beyond the host API.

## Layering

```
cyber-workflow (no runtime dependency)
  module.rs     source, sha256 digest, forbidden imports, origin
  scan.rs       JS tokenizer (strings, templates with nested substitutions, regexes, comments)
  meta.rs       pure-literal `export const meta` extraction and validation
  discovery.rs  global -> project roots, override by meta.name, bounded, no links
  engine.rs     per-run QuickJS Runtime/Context, limits, scheduler, WorkflowHost trait
  prelude.js    determinism guards, code-generation removal, frozen host API globals
  diagnostic.rs typed Diagnostic { kind, message, position: line:col }
cyber-core/config/workflows.rs   `workflows` settings (validated in config load)
cyber-server/runtime/workflows.rs (increment 2) durable run aggregate, implements WorkflowHost
cyber-tools BuiltinHost (increment 3)  agent spawning beside subtask_request
```

The engine depends only on a `WorkflowHost` trait with the methods `agent`, `checkpoint`, `ask` (default: rejects with `NoInteractiveUser`), `phase` and `log`. The trait does not depend on the server, so engine tests use a fake host. The server implementation in increment 2 makes every host call durable before it resolves.

## Decision: JavaScript engine is rquickjs, pinned to `=0.14.0`

**Options.**

- **rquickjs 0.14.0.** Wraps quickjs-ng. Licensed MIT (rquickjs, rquickjs-core and rquickjs-sys; quickjs-ng is MIT).
  - It ships pre-generated bindings for `x86_64`/`aarch64` `apple-darwin`, `x86_64`/`aarch64` `unknown-linux-gnu`, `x86_64`/`aarch64` `unknown-linux-musl`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` and `x86_64-pc-windows-gnu`. That covers every target in `.github/workflows/ci.yml` (the release matrix and the windows-2025 MSVC jobs) with no libclang or bindgen.
  - The C sources build through `cc`, the same way `rusqlite`'s `bundled` feature already does in this workspace. The musl jobs already export `CC=musl-gcc`.
  - The cross-target Windows clippy check uses the stub mingw compiler. That works because only the bindings are type-checked.
  - It provides `set_memory_limit`, `set_interrupt_handler`, `set_max_stack_size`, ES modules, and promise and job control.
- **boa_engine.** Pure Rust, so it would avoid the C toolchain. However, it has no heap limit and no interrupt handler. A busy loop or a large allocation could only be bounded by running the engine on a separate thread and killing the process, which is not possible inside the shared server. Choosing boa would have required spec deltas weakening the `workflows.max_memory_mb` and `workflows.script_cpu_seconds` contracts.

**Choice: rquickjs.** It meets the spec without deltas, and its C build is no heavier than the existing `rusqlite` bundled build.

- The dependency is pinned exactly (`=0.14.0`, `default-features = false`, `std` only). The `futures`, `loader`, `parallel` and `bindgen` features are off.
- QuickJS values are thread-bound, so the engine future is `!Send`. The server runs each run on a dedicated thread or local task (increment 2).
- `cargo deny` is not configured in this repository. The license check is manual and recorded in this document.

## Decision: static meta parser is a hand-written literal scanner (not oxc)

`oxc_parser` would give a full AST, but it pulls in a large dependency tree (allocator, AST, diagnostics, semantic) on a machine with about 9 GiB free and in a CI cache shared by six release targets. The spec only needs two things:

1. Reading a pure literal: objects, arrays, strings, templates without substitutions, numbers, booleans and `null`.
2. Finding forbidden imports.

`scan.rs` is a small tokenizer that handles the lexical traps that matter: strings with escapes, template literals with nested `${}`, the regex-versus-division heuristic, and comments. As a result, `require(` inside a string is ignored, while one inside a template substitution is still found.

- `meta.rs` locates `export const|let|var meta =` at brace depth 0. It then reads a literal and requires the statement to end, so `{...}.x` and `{...} || {}` are rejected.
- Any variable, call, spread, computed key, `+`, or template substitution fails with `ParseError: meta must be a pure literal` at the token's `line:col`.
- Required fields:
  - `name` must match `^[a-z0-9][a-z0-9._-]{0,63}$`.
  - `description` must be non-empty.
  - `phases`, if present, must be `{ title }` objects.
  - `args`, if present, must be an object.
- Other keys are kept in `raw` for later milestones, such as budgets and tool allowlists.

Syntax errors in the rest of the script come from QuickJS itself. They are reported as `ParseError` with the line and column that QuickJS reports.

## Decision: TypeScript is deferred (spec delta)

The spec said "TypeScript transpiled with swc". The swc crates are heavy and fast-moving, so pinning them for every target is costly at this milestone. The options for a later change are `swc_ts_fast_strip` (type stripping only) or `oxc_transformer`. Until then:

- Discovery lists `.ts` files as issues with `UnsupportedSyntax: TypeScript Workflows are not supported yet; save the script as .js`.
- Inline scripts must be JavaScript.

## Sandbox and determinism

Each run creates a new `Runtime` and `Context::full`:

- **Heap limit.** `set_memory_limit(max_memory_mb)`. When allocation fails, QuickJS raises `out of memory`. The engine reports `ExecutionFailure: memory limit of N MiB exceeded`. If the error itself cannot be described because memory is exhausted, the same diagnostic is used.
- **CPU limit.** An `Instant` clock is started and stopped around every synchronous JavaScript slice: prelude installation, module evaluation, each drain of the job queue, and each settlement of a host result. The interrupt handler compares the accumulated time plus the current slice against `script_cpu_seconds`. Time spent awaiting host futures is never charged. Thread CPU clocks are not used, because they are not portable. Once the limit is exceeded, every later interrupt check also returns true. The run fails with `Timeout`, and in-flight host futures are dropped, which cancels them.
- **No module loader.**
  - `import`, `export … from` and `require(…)` are rejected statically with `UnsupportedSyntax` naming the specifier. If no specifier can be read, the error says "dynamic import" or "dynamic require".
  - No resolver or loader is installed, so a computed import that escapes the scan still fails at link time. That failure also maps to `UnsupportedSyntax`.
- **Prelude.** The prelude is `prelude.js`, evaluated in strict mode as a function that receives the native bridge, so the bridge itself is never global.
  - It deletes `eval`, `Function`, `WebAssembly`, timers, `queueMicrotask`, `WeakRef`, `FinalizationRegistry`, `SharedArrayBuffer` and `Atomics`. WeakRef and FinalizationRegistry make garbage-collection timing observable. `Atomics.wait` can block outside the interrupt handler.
  - It sets the `constructor` property of the plain, async, generator and async-generator function prototypes to `undefined`, which closes the code-generation back doors.
  - It replaces `Date` with a frozen wrapper that throws for `Date()` and for argument-less `new Date()`. `Date.now`, `Math.random` and `performance.now` throw `ExecutionFailure: non-deterministic API; pass values through args`. `Math` is frozen.
  - It installs `agent`, `ask`, `checkpoint`, `phase`, `log`, `parallel`, `pipeline`, `args` and `ctx` as non-writable, non-configurable globals. `args` and `ctx` are deep-frozen. `tools` is undefined until increment 3.
- **Typed errors.** Errors created by the host carry `kind`. A private `WeakSet` marks them, so a script cannot forge a `BudgetExceeded` or `AgentFailed` kind, which would change exit codes. Scripts can catch host errors and branch on `e.kind`.
- **Positions.** The engine reads `line:col` from the first `workflow.js:L:C` frame of the stack. For determinism errors, that is the caller's frame, not the prelude's.

## Scheduler

The prelude queues host calls through `native.request(kind, json)`, which returns an id, and keeps the promise's `resolve` and `reject` functions in a private map. The engine loop:

1. Run pending jobs (charged).
2. Dispatch queued `log` and `phase` operations to the host, in order, and turn each call into a host future in a `FuturesUnordered`.
3. If the main promise has settled, return its JSON value or its diagnostic.
4. Otherwise await the next host future (not charged), then settle that call through the private `settle(id, ok, json)` hook (charged).

If the main promise is still pending and no host call is in flight, the script can never finish, and the run fails with `ExecutionFailure`. At most 10,000 host calls may be pending at once.

Host futures receive only the validated agent options (`agent`, `model`, `schema`, `isolation`, `label`, `phase`, `tools`, `budget`, `goal`). Any other key rejects the call. The log is capped at 1 MiB and then ends with a marker.

## Discovery

- Roots are scanned lowest precedence first:
  1. `<config>/workflows` (`~/.config/cyber/workflows`, `scope: global`).
  2. Each `.cyber/workflows` from the project root (`config::project_root`, the nearest `.git` ancestor) down to the Location (`scope: project`).
- `CYBER_DISABLE_PROJECT_CONFIG` skips project roots, as it does for command discovery.
- Workflows are keyed by `meta.name`. A later root replaces an earlier entry, and the replaced paths are listed as `shadowed`.
- Traversal is bounded to 4,096 entries and depth 16, never follows symbolic links (each one is reported as an issue), and reads files of at most 1 MiB.
- A file that fails to load is an issue that carries its diagnostic. It does not hide other Workflows.
- Listing never executes a script.

## Config

`workflows` settings, validated during config load, and their defaults:

| Key | Default | Allowed values |
|---|---|---|
| `max_concurrent` | 16 | 1–256 |
| `max_memory_mb` | 256 | 16–4096 |
| `script_cpu_seconds` | 60 | 1–3600 |
| `tiers` | `fast` → `role:small`, `smart` → `role:default`, `verify` → `role:advisor` | tier name → `provider/model[#variant]` or `role:<default\|small\|advisor\|evaluator\|compaction\|title>` |
| `auto` | `suggest` | `off`, `suggest`, `on` |
| `auto_threshold_files` | 10 | 1–100,000 |
| `size_guideline` | 10 | 1–1000 |

Additional rules:

- Unknown keys are errors.
- Tier names must match `^[a-z][a-z0-9_-]{0,31}$`. Names are open because provider-catalog allows names such as `strong`. An invalid tier name or an unknown role is rejected.
- The effective concurrency is `min(max_concurrent, CPU cores)`.

## Surfaces in increment 1

- **`cyber workflows list`.** Prints a table, or with `--format json` returns `{workflows: [{name, description, phases, args, scope, path, digest, shadowed}], issues: [{path, diagnostic}]}`. `run`, `runs`, `show`, `pause`, `resume`, `stop` and `logs` are parsed by clap and exit with code 2: "`cyber workflows <sub>` is not available in this build yet".
- **HTTP.** `ANY /api/v1/workflows` and `ANY /api/v1/workflows/{*rest}` answer `503 {"_tag":"ServiceUnavailableError","service":"workflow"}` after the normal origin and authentication guard. They are not added to OpenAPI until increment 4 defines the real operations.

## Durable model (increments 2–3; recorded here so increment 1 does not preclude it)

**Aggregate and events.**

- Each run is a new aggregate, `run_<ulid>`. Its events:
  - `workflow.run.{started,paused,resumed,completed,failed,stopped}.1`
  - `workflow.agent.settled.1`
  - `workflow.call.recorded.1` (`ask`, `tools`, `checkpoint`)
  - phase and log events
- The event kinds are registered in `runtime/events.rs`. Projectors maintain the `workflow_run` and `workflow_agent` tables. A `cyber-store` migration adds both tables.
- A process-local Control map `{stop, done}` follows the Job pattern. `recover_workflow_runs` runs next to `recover_jobs` in `App::build`. Shutdown marks running runs `paused` before `tools.shutdown()`.

**Replay keys and revisions.**

- The replay key is `sha256(prompt + canonical(options))`, plus an ordinal per key.
- Each revision records the script and configuration digests (the module digest is already computed).
- If a replay diverges, the run fails closed with `WorkflowReplayMismatch`.

**Agent spawning.**

- `ToolHost::workflow_agent` defaults to "unsupported". `BuiltinHost` implements it next to `subtask_request` and synthesizes an `Invocation` whose `operation_key` is derived from the run and call key, so a retry is idempotent.
- Agents run under the run's Mode as a ceiling. A non-interactive `ask` is auto-rejected with `No interactive approver for workflow run`.

## Decisions in increment 2 (runtime core)

- **Every run has an owner Session.** `POST /api/v1/workflows/runs` requires `session_id`. The owner's Location resolves saved Workflows and `workflows` settings, its Mode is the run Mode, and its rules are the run's rules. Agents are children of the owner, so ownership, deletion fencing, depth limits, hooks and usage roll-up reuse the subagent model. Starting a run without a Session (the CLI in increment 4) creates the Session first.
- **`ToolHost::spawn_workflow_agent`** (the design's `workflow_agent`) takes a `WorkflowAgentRequest` and returns `WorkflowAgentOutput` or a typed `Diagnostic`. `BuiltinHost` synthesizes an `agent` invocation for the owner with `background: false`, the `agt_` ID as subagent name and `operation_key = <run_id>:<call_key>`, and runs it through the shared agent path with `SpawnSource::Workflow`. Delegation is checked like a user delegation: deny rules apply, and nothing asks, because starting the run is the user's decision. The run Mode reaches the child through `child_mode`.
- **Concurrency.** The runtime owns one FIFO `tokio::sync::Semaphore`. `App::build` sizes it once from the global `workflows.max_concurrent` and the core count. The run host acquires a permit before calling the host, and the permit travels in the request into `ChildGuard`, which releases it only after the child settles. Workflow agents never take an `agents.max_concurrent` slot.
- **Engine thread.** Each run drives the engine on its own OS thread with `futures::executor::block_on`. Agent calls run as tokio tasks through the captured `Handle`. A drop guard cancels a task whose host future was dropped, so a failed or timed-out run stops its in-flight children. Store appends are synchronous on the engine thread.
- **Ordering and fail-closed records.** The settled record is appended before the host future returns. If the append fails, the host sets `fatal`, and the engine ends the run before settling the promise. Failed `phase` and `log` records fail the run directly.
- **Options not yet enforced reject.** `goal` (M2.3), `tools`, `budget` and `isolation: "worktree"` reject with `ExecutionFailure`, and so do `checkpoint()` and `ask()`, instead of being silently ignored.
- **Tiers** are resolved per Location when the run starts. `role:<role>` uses `tier_role_ref`, which falls back to the default model, so `verify` works without an advisor. A tier with no model inherits the owner's model. A `model` containing `/` is a model reference, and anything else names a tier. An unknown tier is `ModelUnavailable`.

## Decisions in increment 3 (replay, lifecycle, run budgets)

- **Replay delivers recorded results in recorded order.** Each host call has an issue sequence number per revision and records the order its result reached the script. A same-revision relaunch delivers the recorded prefix in that order, so a deterministic script reaches the same state, including in `pipeline` and limited `parallel` groups, whose issue order depends on settle order. The engine only polls host futures while the script is idle, with every queued call issued. A recorded result whose call was never issued at that point is therefore a divergence, and so is an issued call whose key differs from the record at its sequence number. Both fail with `WorkflowReplayMismatch`.
- **The prefix ends at the first recorded failure,** so resuming a failed run retries the failed agent. `workflow.run.resumed.1` drops the delivery order of results after that point; their values stay reusable by key.
- **After the prefix, and in a new revision, calls match by key.** Completed agent results and checkpoint values of any revision are reused (and recorded into the current revision), an unsettled started agent resumes its child, and anything else runs anew. A checkpoint's key is its name, so it returns its recorded value even when the script now computes another one.
- **Revisions are explicit.** The script digest and a configuration digest (memory and CPU limits and tiers) are recorded per revision with the source. A changed digest without `revision: true` is refused, and the run keeps its state. Budget changes do not need a revision; resume can override budget fields.
- **Logs and phases repeat exactly through the replayed part,** so a relaunch does not record the first N of them again, where N is the number this revision already recorded, until a result the record lacks reaches the script.
- **Pause and stop are durable at once, and engines wind down.** Pause lets in-flight agents settle and be recorded; the next new `agent()` ends the attempt once nothing is in flight. Stop and shutdown cancel in-flight agents and never record their cancellation, so a resume continues their children. Stop also stops each unsettled child's subtree. Resume reopens such a child with its stop receipt (`reopen_subtree`) before `resume_child`, and the child's Drain reconciles its interrupted tool calls (`ToolHost::reconcile`) before the next Turn.
- **The process-local Control counts leases** (the engine thread and each agent task, which owns the child's permit, `ChildGuard` and ownership claim until it returns). A new attempt is refused with `Run has agents still exiting; retry shortly` while any lease is held or an agent child's Drain still runs.
- **Run budgets join the existing Turn check.** `session.workflow_run_id` became a column, and the budget scopes of a Session add the run budget of any ancestor that carries a run ID. The before-Turn check therefore stops agent children and their descendants. Signals for run scopes live in `workflow_run_budget_signal`, because `session_budget_signal` references Sessions. `max_wall_seconds` counts running time only: the budget row accumulates finished running segments. `max_agents` and the other limits are also checked before each new agent starts. The first exhausted limit makes the run end `budget_exceeded`, even when the script catches the rejection and returns a partial result.
- **`ctx.budget` is live, not replayed.** A script that branches on it may diverge on replay, which fails closed.

## Risks

- **Engine CVEs.** QuickJS is C code. Mitigations: the exact pin, a per-run Runtime, heap and CPU limits, and no I/O primitives. Upgrades are deliberate changes.
- **CPU limit precision.** Wall-clock slices over-charge when the machine is under load. Scripts are meant to be thin orchestration, and the 60 s default is generous.
- **Tokenizer gaps.** The regex heuristic can misread exotic code such as `a = b\n/re/.test(x)`. The static scan is defense in depth: the absence of a loader still blocks imports.

## Decisions in the gaps increment

- **`ask()` answers in advance** come from `args.answers`, keyed by the question text. `waiting_input` is entered and left with `workflow.run.waiting.1` and `workflow.run.answered.1`; waiting is not running time for `max_wall_seconds`. A pause or stop withdraws the question and leaves the call unrecorded.
- **Direct tools are host calls of kind `tool`**, recorded with `workflow.call.recorded.1` like checkpoints (M2.2's `workflow.host.settled.1` was not added). The engine, not the prelude, checks the allowlist, so a script cannot widen it. Scripts get wider listing limits than the model (`glob` default 1,000 paths, up to 10,000), in path order.
- **Warm-up groups** are keyed by agent profile, model and result schema, which fix the system prompt and tools. A follower waits before taking a scheduler slot, so the leader is never starved. Only hosts with real child Sessions warm (`ToolHost::warms_prompt_cache`), and `workflows.prompt_cache_warmup: false` turns it off.
- **Runs are not mirrored as `job` rows.** `/tasks` and `/stop` read runs from the run API.
