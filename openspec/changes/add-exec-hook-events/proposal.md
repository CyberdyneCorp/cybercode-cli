## Why

`exec-mode` promises `--include-hook-events` for `stream-json`, but the flag was never built. Hook receipts are their own durable aggregates (`hook.started.1`/`hook.executed.1` on `hke_…`), not Session events, and the live receipt summary added for telemetry (`LiveEvent::HookReceipt`) is dropped by every client stream. So the exec stream had no reliable source for these lines (the note on `complete-daily-coding-surfaces` task 6.3). Automation that runs `cyber exec` in CI wants to see which hooks ran for its run, which of them blocked something, and in what order, without parsing stderr notices.

## What Changes

- `server-api` "Instance event stream": `GET /api/v1/event?include=hook_receipts` also carries `session.hook.receipt`, the IO-free receipt summary the runtime already publishes when a receipt is admitted and when it settles. Without `include=hook_receipts` no stream changes. Session replay, WebSocket and MCP streams still never carry it.
- `hooks` (added requirement "Hook receipt summaries for clients"): what the summary holds and never holds (no stdin, stdout, stderr, decision reason, updated input, additional context or environment, even with `telemetry.log_hook_io`), and that it is live only, so the durable `GET /sessions/{id}/hook-executions` listing stays the source of record.
- `exec-mode` (added requirement "Hook events in exec"; "Streaming input and partial output" now points to it): `cyber exec --include-hook-events` with `--format stream-json`, in single-prompt and `--input-format stream-json` runs, writes one `hook_event` line per settled hook execution of the run's Session. Lines follow the live stream, so a gating hook's line precedes the line of what it gated; before each `result` line exec reconciles with the durable listing so receipts settled before it subscribed or lost to stream lag still get exactly one line. Any other format exits 2; `--workflow` exits 2.
- Delivers the hook-event clause of `complete-daily-coding-surfaces` task 6.3; that change's `tasks.md` is not edited here.

## Impact

- Code: `cyber-server` (`http/events.rs` opt-in, OpenAPI query parameter), `cyber-client` (`Client::all_events`), `cyber-cli` (`exec/hook_lines.rs`, `exec/mod.rs`, `exec/driver.rs`).
- API: additive. A new optional `include` query parameter on `GET /api/v1/event` (OpenAPI and the generated TypeScript SDK); the default stream and the `stream-json` schema without the flag are unchanged.
- Docs: `docs/local-core-guide.md`, `docs/features.md`, `docs/cli.md`, `CHANGELOG.md`.
