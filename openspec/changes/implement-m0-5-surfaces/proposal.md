# Implement M0.5: surfaces

## Why

Milestone M0.5 (ROADMAP) makes the durable runtime usable. It adds the OpenAPI server with SSE replay, the TypeScript SDK, the TUI and `cyber exec` with a JSON stream and exit codes. Every surface goes through the same public `/api/v1` routes, so the server stays the single source of behavior.

## What Changes

- `cyber-server::http`, the `/api/v1` router:
  - Health, OpenAPI 3.1 document, Location, Sessions (CRUD, fork, prompt, command, interrupt, agent/model/mode switches, compact, rewind, revert stage/clear/commit, shell), messages, inbox, context, diff, history, permissions and questions.
  - Catalogs: models, agents, tools, tool schema, commands, file search.
  - The instance SSE stream with heartbeat, and the durable Session stream that replays and then follows without gaps.
  - Tagged errors, pagination, persisted `Idempotency-Key` handling, origin/CORS policy, Basic auth with `auth_token` only on stream routes, and compression of responses of 1 KiB or more.
  - TCP, a Unix socket that only accepts the same OS user, and an in-process embedded transport.
  - JSON-RPC 2.0 over `/api/v1/ws` and `cyber serve --stdio`: operation IDs as methods, event subscriptions, and application-registered tools that the server runs in the client.
- The OpenAPI document is generated from the Rust types (`schemars`). It is committed as `sdk/openapi.json`, and a test fails when it drifts.
- `cyber-app` assembles a server: shared store, catalog, tool host, sandbox, snapshots and runtime. It also owns the lifecycle: one server per user through the ownership lock, port 4747 with fallback, a socket, the password file and `server.json` registration, and service start/stop with health checks.
- `cyber-client` is the Rust API client used by the CLI and the TUI. It covers HTTP and embedded transports, tagged errors, idempotency keys with retries, and SSE with resume.
- CLI:
  - `cyber serve`, `cyber service start|stop|restart|status|password` and `cyber api`.
  - `cyber exec` (alias `-p`): prompt assembly from arguments and stdin, background, embedded, ephemeral and attach backends, Session selection, `dont-ask` default with never-ask Session rules, `--command`, `--file`, budgets and timeout, the text/json/stream-json formats and the exit codes.
  - `cyber [project]` launches the TUI, with `-c`, `-r`, `--fork`, `--prompt`, `--agent` and `--embedded`.
- `cyber-tui`:
  - Composer: multiline, history, drafts and `$EDITOR`.
  - `@` file mentions with line ranges, and `/` command autocomplete.
  - `!` shell commands, steer and queue with take-back, and Esc interrupt.
  - Streaming Markdown-lite rendering, collapsible reasoning and tool cards, and colored diffs.
  - Permission and question prompts.
  - Session, model, mode and theme pickers, with 12 themes.
  - Leader-key chords, terminal title, idle notifications and the exit summary.
- Runtime additions:
  - Session rulesets.
  - Per-Session step limits.
  - User shell commands (`!`) recorded for the next Turn.
  - Skills as user commands.
- `@cyber-code/sdk`: generated from `sdk/openapi.json` by `scripts/generate_sdk.py`, with a hand-written runtime. It covers typed operations, tagged errors, idempotent retries, resumable streams, prompt-and-wait, permission and question handlers, attach/spawn/embedded modes and application-registered tools. CI checks generation drift, typechecks the code and examples, and runs the tests.

## Impact

Several items are deferred:
- PTY routes, mDNS publishing and the generated Rust SDK. The internal `cyber-client` serves the CLI and TUI meanwhile.
- In the TUI: inline rendering mode, mouse support, OSC 52 clipboard, the first-run wizard, the update check, split diffs, syntax highlighting, configurable keybindings, Session export and child-Session trees.
- In the SDK: the Unix socket transport, the `system_prompt`, `append_system_prompt` and `tools` prompt options (the server has no Session system-prompt override yet), and runtime schema validators.
