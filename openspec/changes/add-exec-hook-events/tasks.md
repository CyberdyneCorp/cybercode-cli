## 1. Server

- [x] 1.1 Forward `LiveEvent::HookReceipt` as `session.hook.receipt` on `GET /api/v1/event` only with `include=hook_receipts`, under the same Location filtering; no other stream changes. Declare the `include` query parameter in OpenAPI and regenerate `sdk/openapi.json` and the TypeScript SDK. Test: `crates/cyber-server/tests/http.rs::event_stream_forwards_hook_receipts_only_when_asked` (default, all-Location and Session streams still drop it; the opted-in stream carries the admitted and settled summaries, before a later notice, without IO).

## 2. Client and exec

- [x] 2.1 `Client::all_events(hook_receipts)` opens the all-Location stream with or without the opt-in.
- [x] 2.2 `cyber exec --include-hook-events`: usage error (exit 2) without `--format stream-json` or with `--workflow`; `exec/hook_lines.rs` writes one `hook_event` line per settled receipt of the run's Session from the live stream, reconciles with `GET /sessions/{id}/hook-executions` when it subscribes and before each `result` line (single-prompt, refused prompt and driven runs) and before `end`, fences off a reused Session's earlier receipts, and copies only summary fields with credential redaction of free text. Unit tests in `exec/hook_lines.rs`.
- [x] 2.3 Real-process tests in `crates/cyber-cli/tests/exec_hook_events.rs`: allowing and blocking `PreToolUse` command hooks and a `SessionStart` hook, each line before what it gates, with a secret in hook environment, stderr and decision reason (recorded IO on) never on stdout; no lines without the flag; exit 2 for `json` and `text`; a driven run; a reused Session on the background server reports only its own receipts.

## 3. Docs

- [x] 3.1 `docs/local-core-guide.md`, `docs/features.md`, regenerated `docs/cli.md`, `CHANGELOG.md`.
- [x] 3.2 This change delivers the hook-event clause of `complete-daily-coding-surfaces` task 6.3 (that change's `tasks.md` is left to its own owner).

## 4. Not covered

- [ ] 4.1 Hook lines for child Sessions (subagents, delegated `@agent` prompts, Workflow agents) and for receipts still running when the run ends.
