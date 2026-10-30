## Why

The agent can read code, run commands and ask language servers about types, but it cannot look at a running program. When a test fails for a reason the source does not show (wrong data at runtime, an unexpected branch, a value that changes between two calls), the model falls back to adding `print` lines, re-running, and removing them again. That loop is slow, edits files that should not change, and often misses the moment that matters.

Every mainstream editor solves this with the Debug Adapter Protocol (DAP): one client talks to per-language debug adapters (debugpy, lldb-dap, delve, js-debug) over the same `Content-Length` framing that LSP uses. Cyber Code already has an owned, sandboxed, trust-gated LSP client (`code-intelligence`). A DAP client built the same way lets the model set a breakpoint, run to it, read the stack and variables, evaluate an expression, and stop, without touching the source.

Neither Codex nor OpenCode ships this, and Claude Code only reaches debuggers through third-party MCP servers that run outside its sandbox and permission rules. Doing it natively, inside the same sandbox and permission model as `bash`, is a P2 "better than the alternatives" feature rather than P1 parity.

## What Changes

- **New capability `debugging`.** A DAP client owned by the server, modelled on the LSP client:
  - **Adapters.** Built-in launch recipes for `debugpy` (Python) and `lldb-dap` (C, C++, Rust), found on PATH or in `<cache>/bin`, never downloaded. Custom adapters under `tools.debug.adapters.<id>` with an argv `command`, `languages`, `extensions`, `env` and `launch_defaults`. The first slice speaks stdio transport only.
  - **`.vscode/launch.json`** is read, never written, and only in a trusted checkout. Configurations whose `type` maps to a known adapter can be selected by name; task hooks (`preLaunchTask`, `postDebugTask`) and unknown `${...}` variables are refused.
  - **Trust.** Project adapter definitions and `.vscode/launch.json` are part of the checkout trust digest. In an untrusted checkout they are inactive and nothing from them launches.
- **One `debug` model tool** with an `operation` field, like `lsp`: `launch`, `attach`, `status`, `set_breakpoints`, `set_exception_breakpoints`, `continue`, `step`, `pause`, `wait`, `threads`, `stack`, `variables`, `evaluate`, `terminate`.
- **Session model.** At most one debug session per Session. It is a Job of the new kind `debug` that owns the adapter process tree. It shows in `/tasks`, `task_stop` ends it, and it ends with the Session, on idle timeout (30 minutes), on trust or config revocation, and on server restart (recorded `interrupted`).
- **Permissions and sandbox.** `launch` evaluates the `bash` permission on the rendered program command line, so the same rules, approvals and denies apply as for running it in the shell. The adapter and the program run under the Session's sandbox policy and environment masking. `evaluate` asks by default, because an expression can have side effects. `attach` is limited to processes in the Session's own Jobs and needs a `sandbox.escalate` approval, because a sandboxed adapter cannot reach another sandbox's process.
- **Waiting.** `launch`, `continue`, `step` and `wait` block until the program stops or exits, or until `timeout_ms` (default 30 s, max 300 s). After a timeout the program keeps running, and a later stop is delivered to the Session as a queued notice.
- **Bounded output.** Program stdout/stderr go to the Job log. Each result carries only the new output tail. Variable dumps are bounded by depth (default 1, max 3), count per level (default 50, max 200) and value length (default 512 chars), within the shared tool output budget. Variable handles go stale when the program resumes.
- **Transport and lifecycle.** The same bounded framing as LSP (8 KiB headers, 4 MiB bodies), a 20 s initialize deadline, `terminate` then `disconnect` with a 3 s grace before the process tree is killed, owned cancellation with no request replay, and managed-checkout pins held until native settlement.
- **Surfaces.** `GET /api/v1/sessions/:sessionID/debug` (in the existing `session` route group) and `POST .../debug/terminate`; live `debug.state.changed.1` events; a `debug adapters` line in `cyber doctor`; a compact tool card and a `/tasks` row in the TUI.
- **Registry updates.**
  - `builtin-tools` → Capability-owned tool catalog gains `debug` (debugging).
  - `background-tasks` → Job registry gains the kind `debug`.

## Non-goals

- **A graphical debugger UI**: no source pane, watch panel or breakpoint gutter in the TUI or web client. The user sees tool cards, notices and the `/tasks` row.
- **Reverse or time-travel debugging** (`rr`, `stepBack`, `reverseContinue`). Adapters that advertise `supportsStepBack` are driven forward only.
- **Kernel, firmware and hardware debugging** (kgdb, OpenOCD, JTAG, GDB remote stubs on targets).
- **Socket-transport adapters in the first slice.** `dlv dap` (Go) and `js-debug` (Node) listen on TCP, and `js-debug` also needs `startDebugging` child sessions. Under the Linux network namespace the server cannot reach a port inside the sandbox. They come in a follow-up change, `add-debugger-socket-adapters`, with an in-sandbox stdio bridge. See design.md D2.
- **Remote attach** (host:port, containers, SSH targets) and attaching to processes the Session did not start.
- **`setVariable`, `restartFrame`, `gotoTargets`, data breakpoints, memory and disassembly views.** `evaluate` covers assignment in the first-slice languages.
- **Plugin-contributed adapters.** That follows the `code-intelligence` Code-intelligence plugins pattern, P4.
- **Workflow script access** to `debug` (`tools.*` in QuickJS stays read-only).

## Capabilities

### New Capabilities

- `debugging`: DAP adapters and their trust, the `debug` tool and its permissions, the debug-session Job lifecycle, transport, breakpoints, execution control, bounded inspection, output capture, API, events and doctor reporting.

### Modified Capabilities

- `builtin-tools`: MODIFIED "Capability-owned tool catalog" adds `debug` (debugging). No other line changes.
- `background-tasks`: MODIFIED "Job registry" adds the Job kind `debug`. No other field changes.
- `tui`: ADDED "Debug session presentation".

No requirement in `sandbox`, `permissions-modes`, `workspace-trust`, `hooks`, `server-api` or `cli-commands` is modified. The `debugging` requirements use the contracts those specs already define (the `bash` permission, the `debug` tool-name action, `sandbox.escalate`, the trust digest, `PreToolUse`/`JobEnded` hooks, the `session` route group, `cyber doctor`).

## Impact

- **Code:**
  - new crate module `crates/cyber-core/src/debug/` (or `crates/cyber-dap`): framing (shared with the LSP transport), client, adapter recipes, launch.json reader, session worker;
  - `crates/cyber-tools/src/tools/debug.rs`: the tool, its schema and output formatting;
  - `crates/cyber-server`: Job kind `debug`, routes, events, stop notices, doctor line;
  - `crates/cyber-tui`: tool card and `/tasks` row.
- **Config:** new `tools.debug.*` keys under the existing top-level `tools` key (the `tools.websearch` precedent), so Top-level keys is unchanged. New feature registry entry `debugger` (experimental, off by default).
- **API and SDK:** two routes in the `session` group and one live event type; OpenAPI and the TypeScript SDK are regenerated.
- **Security:** a debug launch is a code-execution path with the same permission and sandbox as `bash`; `evaluate` asks; adapter definitions are trust-gated. Covered by new prompt-injection evaluation cases (task 9.3).
- **Docs:** `docs/debugging.md` (new), `docs/features.md`, `docs/local-core-guide.md` (tool list), `ROADMAP.md` (P2 approved additions) when this change is approved.
