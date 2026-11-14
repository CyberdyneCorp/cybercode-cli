## Context

A hook execution leaves a durable receipt: `hook.started.1` at admission and `hook.executed.1` at settlement, in an independent `hke_…` aggregate projected into `hook_execution` and listed by `GET /api/v1/sessions/{id}/hook-executions`. The receipt may hold raw IO when `telemetry.log_hook_io` is on. Since `add-otel-export`, the runtime also publishes `LiveEvent::HookReceipt { session_id, receipt }` on its bus at admission and settlement; the summary type (`HookReceipt`) has no IO field. Client streams dropped it.

`cyber exec` follows its Session through the instance stream (`/event?scope=all`, filtered by Session ID) and writes `stream-json` lines from what it sees.

## Goals

- One `hook_event` line per hook execution of the run's Session, after it settles.
- Lines in the order the runtime settled the receipts, interleaved with the other lines, so a gating hook's line comes before the line of the action it allowed or blocked.
- No line is lost to stream lag or to a receipt that settled before exec subscribed.
- No hook IO, decision reason, rewritten input, added context or environment in a line.
- Nothing changes for any client, or for `stream-json` output, without the opt-in.

## Non-goals

- Hook lines for other Sessions: subagents, delegated `@agent` prompts (which follow the child Session's history) and Workflow agents. `--workflow` refuses the flag; a delegated prompt simply has no hook lines.
- Receipts still `running` when the run ends (for example an `async` handler) get no line.
- The matcher of the group that selected the handler: the receipt does not record it; the hook ID (or definition digest) and the event identify the handler.
- Carrying receipts on the Session replay stream, WebSocket or MCP notifications.

## Decisions

**Source: the live receipt summary, opt-in on the instance stream.** The summary already exists, is published in settlement order on the same bus as Session events, and has no IO by construction. Forwarding it only when a stream asks with `include=hook_receipts` keeps every existing client unchanged. Making receipts Session events was rejected: it would change replay, history pages and the telemetry mapper, and receipts are deliberately independent aggregates so their settlement cannot race the Session state cache.

**Reliability: reconcile with the durable listing.** The bus is a bounded broadcast; a lagging stream skips events, and receipts that settle before exec subscribes (a `SessionStart` hook while the Session is created) are never live for it. Exec therefore reads `GET /sessions/{id}/hook-executions` when it subscribes (for a Session it created) and before every `result` line, and emits settled receipts that have no line yet, oldest first. Lines are keyed by receipt ID, so a receipt is reported once whichever path sees it first. Only summary fields are copied from the listing, which may carry IO.

**Which receipts belong to the run.** A Session exec creates (or forks) has only this run's receipts. For a reused Session (`--session`, `--continue`), exec records the newest receipt position (`started_ms`, ID; the listing's own order) before the prompt and reports only receipts after it. This uses the server's own clock and order, so `--attach` clock skew does not matter. A receipt admitted before the run but settled during it is not reported.

**Ordering, stated honestly.** A synchronous hook settles its receipt before the runtime applies its decision, and the bus preserves publish order, so on the live path a `PreToolUse` line precedes that call's `tool_result`, a `UserPromptSubmit` or `SessionStart` line precedes the answer, and a `Stop` line precedes `result`. Lines recovered by reconciliation come late but still before the `result` they belong to (or before `end`). There is no ordering guarantee across Sessions.

**Line shape.** `{"type":"hook_event","receipt_id","hook_id","event","scope","outcome","decision","duration_ms","started_ms","tool_name"?,"call_id"?}` plus the usual `ts` and `session_id`. `decision` is the decision kind (`allow`, `ask`, `deny`, `block`, `retry`) or null. `hook_id` is the configured ID or the definition digest. The two free-text fields, `hook_id` and `tool_name`, pass through the built-in credential redaction. The line is an opt-in addition; the `init` line's `schema_version` stays 1 for single-prompt runs and 2 for driven runs.

**Flag validation.** Consistent with `--input-format stream-json` and `--include-partial-messages`, a flag that cannot be honored is a usage error before any Session exists: `--include-hook-events` without `--format stream-json` exits 2, and so does `--include-hook-events --workflow`.

## Security

- The summary type has no IO field, so neither the live event nor a line can carry hook stdin, stdout or stderr, regardless of `telemetry.log_hook_io`. Lines are built from an allow-list of summary fields; the decision `reason` (which holds a blocking hook's stderr), `updated_input` and `additional_context` are never copied.
- The opt-in stream needs the same authentication as every other stream, and the summary is a subset of what the authenticated `hook-executions` listing already returns.
- Tool input and output still appear only in the existing `tool_use`/`tool_result` lines, with their existing redaction.
- Hook notices on stderr are unchanged by this change.

## Risks

- A long run on a reused Session with many receipts pays one listing page per `result`; reconciliation stops at the floor and reads at most 20 pages of 500.
- A receipt admitted in the same millisecond as the floor with a smaller ID would be treated as earlier; receipt IDs are time-ordered, so this needs clock regression.
