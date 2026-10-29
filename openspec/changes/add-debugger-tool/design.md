## Context

`code-intelligence` already runs third-party protocol servers for the model: it finds trusted executables, launches them inside the shared sandbox, frames messages with bounded `Content-Length` headers, owns their process trees until acknowledged settlement, pins managed checkouts, fences on trust or config change, and turns protocol results into bounded, Location-relative tool output. The Debug Adapter Protocol uses the same base framing (header + JSON body) but different messages: `{seq, type: "request"|"response"|"event"}` instead of JSON-RPC 2.0, plus reverse requests from the adapter (`runInTerminal`, `startDebugging`).

What differs from LSP, and drives most decisions below:
- **A debug launch runs the user's program.** An LSP server analyses code; a debug adapter executes it. This is the `bash` threat model, not the `lsp` one.
- **State is per run, not per Location.** A language server is shared by every Session in a Location. A debug session belongs to one Session's investigation and has a stopped/running state that two Sessions cannot share.
- **Results are time-dependent.** Variable references are only valid while the program is stopped; the program can stop later, when no tool call is waiting.
- **Values are unbounded and untrusted.** A variable can be a 2 GB buffer or a string that tells the model to do something.

Relevant existing contracts: `background-tasks` (Jobs, process-tree stop, completion notices, `max_jobs`), `sandbox` (policies, network proxy, escalation), `permissions-modes` (rules, Modes, non-interactive behaviour), `workspace-trust` (trust digest), `tool-registry` (output budget, invocation context, abort within 2 s), `hooks` (`PreToolUse`, `JobEnded`), `server-api` (route groups), `tui` (`/tasks`, tool cards).

## Goals / Non-Goals

**Goals:**
- The model can find the line where state goes wrong without editing source.
- A debug launch can do nothing a `bash` call with the same command line could not.
- Every result fits the tool output budget, and every process the debugger starts is owned and settled.
- The implementation reuses the LSP framing, sandbox launch and checkout-pin code instead of copying it.

**Non-goals:** see proposal.md.

## Decisions

### D1. A new `debugging` capability, structured like `code-intelligence`
The requirements mirror the LSP set one for one where the concern is the same: enablement, built-in definitions, custom definitions, trusted launch, managed launch pins, live authority, bounded framing, owned connection lifecycle, status API. They differ where the threat or state model differs (D4, D5, D6).

*Alternative rejected:* adding the requirements to `code-intelligence`. That spec is about analysis servers shared per Location; mixing in code-executing, per-Session sessions would blur both its permission story (`lsp` is offered in plan mode as read-only) and its lifecycle (Location pools).

### D2. First-slice adapters: stdio only (`debugpy`, `lldb-dap`)

| Recipe id | Languages | Detection | Launch command | Why in slice 1 |
|---|---|---|---|---|
| `debugpy` | Python | the Location's interpreter (`.venv/bin/python`, then `python3` on PATH) imports `debugpy` | `<python> -m debugpy.adapter` | stdio, one session, `justMyCode`, exception filters `raised`/`uncaught` |
| `lldb-dap` | C, C++, Rust (and Swift, Zig via custom) | `lldb-dap` (LLVM ≥ 18) or `lldb-vscode` on PATH or in `<cache>/bin` | `lldb-dap` | stdio, ships with LLVM, no extension download |

- Both speak DAP over stdio, launch the debuggee as their own child (so it inherits the sandbox), and never need `startDebugging`.
- **Deferred to `add-debugger-socket-adapters`:** `dlv dap` (Go) listens on TCP; `js-debug` listens on TCP and spawns child sessions via `startDebugging`. On Linux the sandbox puts processes in their own network namespace, so the server cannot dial a port inside it. The follow-up runs a small stdio↔loopback bridge inside the same sandbox, and adds child-session support. Doing it now would double the transport surface of the first slice.
- **CodeLLDB rejected:** it is a VS Code extension with its own launch handshake and TCP transport, and is usually installed by the editor, not on PATH. `lldb-dap` covers the same languages with a plain stdio binary.
- **No downloads.** Like `lsp.auto_install: false`, a missing adapter is reported (`cyber doctor`, tool error with an install hint) and never fetched.

### D3. Custom adapters and `.vscode/launch.json`
- `tools.debug.adapters.<id>`: `{ command: string[], languages?, extensions?, env?, launch_defaults?: object, disabled? }`. Custom ids require `extensions` or `languages`, as custom LSP ids require `extensions`. Only `transport: "stdio"` is accepted in this change; any other value fails config validation with a pointer to the follow-up.
- The key lives under the existing top-level `tools` key, following `tools.websearch.*`. *Alternative rejected:* a new top-level `debug` key. It would modify the Top-level keys registry (a frequent merge point for parallel changes) and read like the unrelated `cyber debug` maintenance command.
- **launch.json** is read with the JSONC parser, never written. A configuration is usable when its `type` maps to a known adapter (`debugpy`/`python` → `debugpy`; `lldb-dap`/`lldb` → `lldb-dap`; anything else is listed as unsupported) and `request` is `launch`. Only an allowlist of fields passes through: `program`, `module`, `args`, `cwd`, `env`, `stopOnEntry`, `justMyCode`, `console` (forced to internal), `name`. Variable substitution is limited to `${workspaceFolder}`, `${workspaceFolderBasename}` and `${file}` (the path given in the tool call); `${env:...}`, `${command:...}` and `${input:...}` refuse that configuration. `preLaunchTask` and `postDebugTask` are ignored with a warning in the result, because they run editor tasks Cyber does not own.
- **Trust.** Project adapter definitions and `.vscode/launch.json` contribute to the checkout trust digest. Untrusted, they are not read into any launch. *Alternative rejected:* relying only on the `bash` permission at launch. The permission prompt shows the program command line but not an adapter `command` or `env` such as `LD_PRELOAD`; those must be reviewed through trust, as hooks and MCP servers are.

### D4. One `debug` tool with operations
`{ operation, ...fields }`, the same pattern as `lsp`. The schema is one object with an `operation` enum and per-operation required fields validated by the tool.

*Alternatives rejected:*
- **Fourteen tools** (`debug_launch`, `debug_step`, …). This adds fourteen names to the single tool registry and to every model's tool list, which costs prompt tokens in every Turn whether or not debugging is used. Permission rules can still tell operations apart through the resource (D6).
- **An MCP server** (e.g. an existing DAP-over-MCP bridge). It would run outside the sandbox unless `sandbox.apply_to` includes `mcp`, and could not share Jobs, notices or permission resources.

### D5. Session model: one debug session per Session, owned by a Job
- A debug session is a Job of kind `debug` (`job_` ID). The Job owns the adapter's process group; the debuggee is the adapter's child, so `Stop kills process trees` reaches it.
- **One per Session.** A second `launch` fails with `A debug session is already active (job_…); terminate it first`. Subagent Sessions may own their own. The server-wide cap is `tools.debug.max_sessions` (default 4). Each debug Job also counts toward `background.max_jobs`.
- **Ends** on `terminate`, `task_stop`, the `/tasks` Stop action, Session deletion, idle timeout (`tools.debug.idle_timeout_minutes`, default 30, counted from the last `debug` call while not waiting), trust or config revocation (D9), and when the program exits. A server restart marks it `interrupted`; it is never resumed, because the process is gone.
- **Interrupt** (Esc) aborts a waiting `debug` call within 2 s but leaves the debug session alive, as interrupting a Drain leaves background Jobs alive.
- *Alternative rejected:* a Location-shared pool like LSP. Two Sessions stepping one program would see each other's stops and stale handles.
- *Alternative rejected:* several concurrent debug sessions per Session. Multi-process debugging is real but needs child-session routing (the `startDebugging` follow-up). One session keeps "the current stop" unambiguous for the model.

### D6. Permissions and sandbox
- **launch** evaluates the `bash` permission once, with the program's rendered command line as the resource (argv shell-quoted, `python -m pytest tests/test_x.py`; for a module launch `python -m <module> <args>`). No shell runs, so the command is one simple command; the offered "always" pattern is its arity prefix plus ` *`, as for `bash`. Existing `bash` allows, denies and Session approvals apply unchanged, `accept-edits` does not auto-allow it, plan mode denies it, `dont-ask` and non-interactive Sessions deny unless a rule allows it. Then the `debug` action is evaluated with resource `launch <adapter>`.
- **Sandbox.** The adapter starts through the shared sandbox launcher with the Session's policy, writable roots, network proxy and credential masking, so the debuggee inherits them. On platforms where enforcement is unavailable, launch fails closed with `SandboxUnavailableError`, as `bash` does. `sandbox.excluded_commands` is not consulted: the adapter, not the program, is what is launched, and an exclusion for `docker *` must not unsandbox a debugger.
- **evaluate** evaluates the `debug` action with resource `evaluate <expression>` (truncated to 200 chars for display, matched on the full text). Default `ask`, "always" pattern `evaluate *` for the Session. An expression is arbitrary code in the debuggee; the sandbox bounds its effect but the approval for `python app.py` did not cover it.
- **Inspection and control** (`status`, `threads`, `stack`, `variables`, `set_breakpoints`, `set_exception_breakpoints`, `continue`, `step`, `pause`, `wait`, `terminate`) are allowed by default through built-in rules on the `debug` action. They act only on a program the user already approved. Logpoint and condition expressions are evaluated by the adapter, so `set_breakpoints` with a `condition` or `log_message` evaluates `debug` with resource `evaluate <expression>` for each expression.
- The tool is annotated `destructive` (classifier review in `auto`) and not `read_only` (hidden in plan mode, denied to read-only agents).
- **attach** is limited to a `pid` inside a process tree owned by a running Job of the same Session (for example a background `bash` dev server). It evaluates `bash` on `attach <pid> <job command>` and then raises a `sandbox.escalate` request with the model's justification, because a sandboxed adapter cannot ptrace a process in another sandbox (Landlock domains, Seatbelt profiles, AppContainer). Approval runs that adapter unsandboxed for that one debug session. `dont-ask` denies the escalation. Under `full-access` no escalation is needed. `terminate` on an attached session detaches (`disconnect { terminateDebuggee: false }`) and leaves the target Job running.
- *Alternative rejected:* a new `debug.launch` permission action independent of `bash`. Users would need to copy every `bash` deny into a second ruleset to keep them effective.

### D7. Stop-event waiting
- `launch`, `continue`, `step` and `wait` take `wait: boolean` (default true) and `timeout_ms` (default `tools.debug.wait_timeout_ms` = 30000, max 300000).
- They return when the adapter sends `stopped`, `exited` or `terminated`, or on timeout. On timeout the result is `{ state: "running" }` with the new output tail, and the program keeps running.
- **Stop notices.** If the program stops or exits while no `debug` call is waiting, the server admits one notice into the Session with `delivery: queue` and `source: debug`, containing the stop summary. At most one stop notice is pending per debug session; a newer stop replaces it. This reuses the Completion notices path.
- The **stop summary** is the same everywhere: reason, thread, `path:line:col` of the top frame (Location-relative), five source lines around it, up to 10 frames, and the first level of the top frame's local variables, at most 20 entries.

### D8. Output capture and bounded inspection
- **Output.** DAP `output` events in categories `stdout`, `stderr` and `console` are appended to the Job log `<data>/jobs/<job_id>.log` as `[stdout] …` lines, under the existing `background.max_output_mb` cap. `telemetry` and `important` go to the server log only. Each `debug` result appends the output produced since the previous result, at most 40 lines and 4 KiB (tail kept), plus the log path for `read`. The debuggee's stdin is closed. Launch forces the internal console (`console: "internalConsole"` for debugpy; `runInTerminal` is refused) so output never reaches a terminal the server does not own.
- **Variables.** `variables { frame?, ref?, depth?, count?, start?, max_value_chars? }`:
  - `depth` default 1, max 3; `count` per container default 50, max 200, with `start` for paging (DAP `start`/`count`, `filter: named|indexed` when the adapter supports it);
  - values truncated to `max_value_chars` (default 512, max 4096) with `…(<n> chars)`;
  - lazy variables (`presentationHint.lazy`) and getters are not auto-expanded;
  - a hard stop at 500 variables per call and at the tool output budget, with `[… <n> more; page with start=<k>]`;
  - every expandable entry carries a handle `v<stop>.<n>`. Handles are only valid for the stop that produced them; after resume they fail with `Stale variable handle: the program resumed after stop <stop>`.
- *Alternative rejected:* returning the adapter's JSON as is. A single numpy array or a C struct with a large buffer exceeds the budget, and raw `variablesReference` numbers silently become wrong after resume.

### D9. Transport, lifecycle and ownership (mirroring LSP)
- **Framing.** Reuse the LSP framing reader/writer: ASCII `Content-Length` headers ≤ 8 KiB, bodies ≤ 4 MiB, UTF-8 JSON objects with `seq` and `type`. A cancelled partial read or write fences both directions.
- **Handshake.** `initialize` (with `clientID: "cyber"`, `linesStartAt1`, `columnsStartAt1`, `pathFormat: "path"`, `supportsRunInTerminalRequest: false`, `supportsStartDebuggingRequest: false`) within 20 s; then `launch`/`attach`; on the `initialized` event, `setBreakpoints` per file, `setExceptionBreakpoints`, then `configurationDone`. Breakpoints given in the `launch` call are sent in that window so the first stop can hit them.
- **Reverse requests.** `runInTerminal` and `startDebugging` get an error response. An adapter that cannot continue without them fails the launch with a clear message.
- **Shutdown.** `terminate` (if `supportsTerminateRequest`), then `disconnect { terminateDebuggee: true }` (false for attach), then after 3 s the Job's process-tree stop (SIGTERM, 5 s, SIGKILL). The Job records `completed` when the program exited by itself, `cancelled` when stopped, `error` when the adapter failed.
- **Cancellation.** The tool abort signal stops waiting within 2 s. An in-flight request is cancelled with DAP `cancel` when the adapter supports it; a late response is matched by `seq` and dropped. Requests are never replayed.
- **Checkout pins.** The debug Job claims every enclosing managed checkout of the Location and the launch `cwd` with its own owner identity before spawn and keeps the claim until process settlement is acknowledged, so a worktree cannot be removed under a running debuggee.
- **Live authority.** Before each operation the session rechecks the trust-filtered adapter definition and launch configuration it was started from. A change or revocation ends the session (D5) and the next call reports why.

### D10. Surfaces
- **API.** `GET /api/v1/sessions/:sessionID/debug` returns `{ active, job_id?, adapter?, state: idle|starting|running|stopped|terminating, stop?: summary, breakpoints: n }`; `POST /api/v1/sessions/:sessionID/debug/terminate` ends it. Both are in the existing `session` route group, so server-api's route-group registry is unchanged. Stopping also works through the existing Job stop paths.
- **Events.** Durable `job.started.1`/`job.ended.1` already describe the lifecycle. One new live-only event, `debug.state.changed.1` `{ session_id, job_id, state, stop? }`, lets clients update without polling. It is not durable, so replay never shows stale stops.
- **Hooks.** No new hook events. `PreToolUse`/`PostToolUse` see `tool_name: "debug"` with the operation in `tool_input`, and `JobEnded` fires for kind `debug`. A hook can block `evaluate` with the existing matcher syntax.
- **CLI.** No new command. `cyber doctor` gains a `debug adapters` line listing each recipe as `ready`, `not installed` or `disabled`. `cyber debug` (maintenance) is unrelated and unchanged.
- **TUI.** The tool card shows `debug <operation>` and a one-line summary (`stopped at src/app.py:42 (breakpoint)`). `/tasks` lists the debug Job with its state as the last line. The Session gets a notice when a debug session starts and ends.

### D11. Phase P2
- It depends on P1 work: sandbox enforcement on all three platforms, background Jobs and notices, the LSP framing and ownership code, and the auto-mode classifier.
- It is not needed for P1 parity: Codex and OpenCode have no debugger.
- It fits the P2 theme (more capable autonomous work) and is gated behind the experimental feature `debugger` until the acceptance in tasks section 9 passes.

## Risks / Trade-offs

- **Adapter quirks.** debugpy and lldb-dap differ in exception filters, thread naming and `evaluate` contexts. → Recorded-transcript fixtures per adapter (tasks 2.4, 3.3), and capability-driven behaviour (`supports*` flags) instead of adapter-name checks.
- **Sandboxed debuggers can fail in surprising ways.** lldb on macOS needs the `com.apple.security.cs.debugger` entitlement path (`DevToolsSecurity`) and ptrace of its own child; Seatbelt must allow `process-info*` and `mach-task-name` for the child. → The sandbox profile gains only the rules for the adapter's own descendants; `cyber sandbox test` gains a debugger probe (task 4.4); failure is reported as `Sandbox denied: debug <operation>` rather than as a hang.
- **Evaluate prompts can be noisy.** → The "always" pattern `evaluate *` is per Session; `auto` mode routes them to the classifier.
- **Prompt injection through values.** A string variable can contain instructions. → Values are labelled as untrusted data from the debuggee, terminal controls are escaped, and evaluation cases cover it (task 9.3).
- **Several open changes modify the catalog list.** `add-binary-hex-editor` (`binary`) and `add-repository-map` (`repo_map`) also MODIFY Capability-owned tool catalog. This delta is written against `main`. → Whichever archives later rebases its delta onto the archived text and keeps every added line; the rest of the requirement is unchanged.
- **Go and Node users wait for the follow-up.** → Custom stdio adapters work meanwhile; the follow-up is the next change on this capability.

## Migration Plan

- Purely additive. The feature `debugger` is off by default; with it off, `debug` is not advertised and `tools.debug.*` keys are validated but unused.
- No stored data changes beyond the new Job kind, which older readers display as an unknown kind. Rollback is turning the feature off.
- Enabling it in a trusted checkout with `.vscode/launch.json` changes the trust digest once, so the user reviews the launch configurations the first time.

## Open Questions

- Should `stack` return source previews for every frame or only the top one? The first slice returns only the top one, to stay within the budget.
- Should a stop notice wake an idle Session (start a Turn) or wait for the next user prompt? The first slice queues it with `delivery: queue`, which starts a Turn like Job completion notices; a config switch can come later if it proves noisy.
