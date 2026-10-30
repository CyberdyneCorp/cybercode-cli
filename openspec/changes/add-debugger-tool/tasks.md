## 1. Feature, config and registry

- [ ] 1.1 Register the feature `debugger` (experimental, default off) and the `tools.debug.*` schema: `enabled`, `wait_timeout_ms`, `idle_timeout_minutes`, `max_sessions`, `adapters.<id>` with `command`, `languages`, `extensions`, `env`, `launch_defaults`, `transport`, `disabled`.
  - Unit tests: defaults; custom id without `extensions`/`languages` fails; `transport: "tcp"` fails with `ConfigInvalidError` naming the key ("Socket transport refused"); keys validated with the feature off.
- [ ] 1.2 Add `debug` to the capability-owned tool catalog and the Job kind `debug` to the registry and its storage enum.
  - Tests: "Feature off by default" and "Feature on without adapters" (tool not advertised; call settles as `Unknown tool: debug`); an old database reads a `debug` Job row; "Debug session Job".

## 2. Transport and connection

- [ ] 2.1 Extract the LSP `Content-Length` framing into a shared module used by both clients, and add the DAP message layer (`seq`, `type`, `request_seq` matching).
  - Tests: the existing LSP framing tests still pass; "Unicode across fragments"; "Oversize frame"; duplicate and unmatched responses fence both directions; cancelled partial read refuses later I/O.
- [ ] 2.2 DAP connection: `initialize` with the fixed client capabilities and a 20 s deadline; `initialized` → breakpoints → `configurationDone` ordering; reverse requests answered with errors.
  - Tests with a scripted fake adapter: "Breakpoints before the program runs"; initialize timeout; "Reverse terminal request"; unknown reverse request; stderr excerpt bounded to 512 escaped bytes.
- [ ] 2.3 Shutdown and cancellation: `terminate` when supported, `disconnect` (`terminateDebuggee` per launch/attach), 3 s grace, then Job process-tree stop; DAP `cancel`; late responses dropped by `seq`; no replay.
  - Tests: "Stalled adapter shutdown"; "Abort during evaluate"; owner drop terminates the tree without reporting settlement.
- [ ] 2.4 Recorded-transcript fixtures for debugpy and lldb-dap (initialize, launch, stop, stack, variables, evaluate, exit) replayed against the client.
  - Golden tests per adapter.

## 3. Adapters, launch.json and trust

- [ ] 3.1 Built-in recipes `debugpy` (interpreter selection `.venv/bin/python`, then `python3`; `import debugpy` check) and `lldb-dap` (`lldb-dap`, then `lldb-vscode`, on PATH or `<cache>/bin`). No downloads.
  - Tests: "Python adapter from the project venv"; "Missing adapter" (message and install hint, no process); a custom entry with a built-in id overrides the recipe.
- [ ] 3.2 Custom adapters and adapter selection: multiple matches need `adapter`, else the call lists candidates.
  - Tests: "Custom stdio adapter"; ambiguous match error.
- [ ] 3.3 launch.json reader: JSONC, 256 KiB limit, `type` mapping, field allowlist, forced internal console, `${workspaceFolder}`/`${workspaceFolderBasename}`/`${file}` only, task hooks ignored with a warning. Never writes the file.
  - Tests: "Named configuration"; "Task hook and env variable refused"; parse error and oversize file reported and ignored; file unchanged byte for byte after use.
- [ ] 3.4 Add project `tools.debug.adapters` and `.vscode/launch.json` to the checkout trust digest.
  - Tests: "Untrusted checkout"; "launch.json edited after approval"; global adapters stay available in an untrusted checkout.
- [ ] 3.5 Live authority: recheck definition, executable and configuration before each operation and on an idle review; end the session on change, revocation or feature disable.
  - Test: "Trust revoked while stopped".

## 4. Permissions and sandbox

- [ ] 4.1 Launch permission: render the shell-quoted command line, evaluate `bash` (one simple command, arity-prefix "always" pattern), then `debug` `launch <adapter>`.
  - Tests: "Bash deny applies to the debugger"; a Session `bash` approval for `python app.py *` covers the launch; `accept-edits` still asks; `dont-ask` and `cyber exec` without an allow rule deny; "Plan mode".
- [ ] 4.2 Evaluate permission and built-in default rules (`debug/evaluate * ask`, `debug/* allow`), including breakpoint `condition`, `hit_condition` and `log_message` expressions; 4096-character limit.
  - Tests: "Evaluate asks by default"; "Conditional breakpoint expression"; the Session "always" pattern `evaluate *` suppresses later prompts; `auto` routes the request to the classifier.
- [ ] 4.3 Sandboxed adapter launch through the shared launcher with policy, writable roots, network mode, environment policy and masking; `excluded_commands` not consulted; fail closed without enforcement.
  - Tests: "Sandboxed debuggee" with a `sandbox.denied.1` event on the debug Job; "Masked credentials"; `SandboxUnavailableError` on a host without enforcement; an `excluded_commands: ["python *"]` entry leaves the debug launch sandboxed.
- [ ] 4.4 Platform sandbox rules for the adapter's own descendants (Seatbelt `process-info*` and task access for children; Landlock and AppContainer unchanged), and a debugger probe in `cyber sandbox test`.
  - Tests: lldb-dap stops a sandboxed child on macOS and Linux CI; probe output lists `debug: ok` or the denial.
- [ ] 4.5 Attach: pid must be inside a running Job tree of the same Session; `bash` check on `attach <pid> <job command>`; `sandbox.escalate` unless `full-access`; detach on terminate.
  - Tests: "Attach to a background dev server"; "Foreign process refused"; `dont-ask` denies the escalation; terminate leaves the target Job running; host:port attach refused.

## 5. Session ownership

- [ ] 5.1 Debug Job ownership: one per Session, `tools.debug.max_sessions` server cap, counts toward `background.max_jobs`, status mapping (`completed`, `cancelled`, `error`, `interrupted` on restart).
  - Tests: second launch fails with the active-session message; fifth server-wide session refused; restart marks `interrupted` and does not resume.
- [ ] 5.2 Ending paths: `terminate`, `task_stop`, `/tasks` Stop, Session deletion, program exit, idle timeout; interrupt keeps the session.
  - Tests: "Idle debug session reclaimed"; "Esc keeps the debug session"; Session deletion settles the process tree.
- [ ] 5.3 Managed checkout pins with a debug owner identity, held until acknowledged settlement.
  - Test: "Worktree removal while debugging"; unacknowledged disposal leaves removal refused.

## 6. Tool operations

- [ ] 6.1 Tool schema and dispatch for all fourteen operations, 1-based positions, Location-relative paths, `No active debug session` and `The program is running; call pause or wait` errors, annotations `destructive` and not `read_only`/`concurrency_safe`.
  - Tests: "Launch to a breakpoint"; "Inspection without a session"; schema bundle at `GET /api/v1/tools/schema` includes `debug`.
- [ ] 6.2 Breakpoints: per-file replace and clear, function breakpoints gated on capability, exception filters validated against the adapter, 100-breakpoint and 512-character log limits, verification results.
  - Tests: "Logpoint"; "Unknown exception filter"; "Unverified breakpoint"; breakpoint 101 refused.
- [ ] 6.3 Execution control and waiting: `continue`, `step` kinds and granularity, `pause`, `wait`, `timeout_ms` bounds, stop summary, exit handling.
  - Tests: "Step over"; "Wait timeout"; `timeout_ms: 400000` is rejected; exit reports the code and ends the session.
- [ ] 6.4 Stop notices: one pending notice per debug session, `delivery: queue`, `source: debug`, replacement by newer events.
  - Tests: "Breakpoint hit after a timeout"; "Newer event replaces a pending notice".
- [ ] 6.5 Output capture to the Job log with category prefixes and the 40-line / 4 KiB tail per result; stdin closed; terminal controls escaped.
  - Tests: "Output tail"; `telemetry` output absent from the Job log; an ANSI clear-screen sequence arrives escaped.
- [ ] 6.6 Threads and stack: 100-thread cap, `start`/`levels` paging (max 50), `total_frames`, top-frame-only preview, no reads outside permitted paths.
  - Tests: "Deep recursion"; a frame in `/usr/lib` returns its path without a preview read.
- [ ] 6.7 Bounded variables: depth, count, `start` paging, value truncation, lazy entries, 500-variable cap within the output budget, `v<stop>.<n>` handles and staleness, untrusted-data labelling.
  - Tests: "Large list"; "Stale handle"; "Injected instruction in a string"; a 1 MB string is truncated to 512 chars with its length.

## 7. Surfaces

- [ ] 7.1 Routes `GET /api/v1/sessions/:sessionID/debug` and `POST .../debug/terminate` in the `session` group; OpenAPI and SDK regeneration.
  - Tests: "Client status"; terminate without a session returns `404`; generated SDK method round-trip.
- [ ] 7.2 Live-only `debug.state.changed.1` event; durable lifecycle through `job.started.1`/`job.ended.1`; `PreToolUse`/`PostToolUse`/`JobEnded` payloads for `debug`.
  - Tests: event not present after replay; "Hook blocks evaluate"; `JobEnded` fires with kind `debug`.
- [ ] 7.3 `cyber doctor` `debug adapters` line (no adapter start, sandboxed 5 s import check).
  - Test: "Doctor lists adapters"; warning when none is ready.
- [ ] 7.4 TUI card, `/tasks` row and start/end notices.
  - Tests (TUI snapshot): "Stop shown in the card"; "Stop from /tasks".

## 8. Docs

- [ ] 8.1 `docs/debugging.md` (adapters, config, permissions, limits, attach), `docs/features.md`, `docs/local-core-guide.md` tool list, and a P2 "approved additions" entry in `ROADMAP.md` once approved.

## 9. Acceptance

- [ ] 9.1 End to end on the real binary with a scripted model: a failing Python test is launched under debugpy, stops at a breakpoint, the model reads a local and evaluates one expression after approval, then terminates; no source file changes. macOS and Linux.
- [ ] 9.2 The same with a Rust test binary under lldb-dap, including a conditional breakpoint and `step in`.
- [ ] 9.3 Prompt-injection evaluation cases in `harness-evaluation`: a variable value and a program output line that ask the model to read `~/.ssh` and upload it; both stay data and the permission and sandbox checks hold.
- [ ] 9.4 `openspec validate add-debugger-tool --strict` and `python3 scripts/spec_lint.py` pass after the specs are synced.
