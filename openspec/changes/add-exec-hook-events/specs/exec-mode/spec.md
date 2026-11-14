## MODIFIED Requirements

### Requirement: Streaming input and partial output
(P1) `--input-format stream-json` SHALL read JSON lines from stdin for the life of the process: `{ "type": "user", "text" | "parts", "delivery"? }` admits a prompt, `{ "type": "interrupt" }` interrupts the Drain, `{ "type": "permission_reply", "request_id", "reply", "message"? }` and `{ "type": "question_reply", "request_id", "answers" }` answer pending requests (which are emitted as `permission_request` and `question` events instead of being auto-denied), and `{ "type": "end" }` or EOF ends the run after the current Drain goes idle. Each prompt SHALL produce its own `result` event and the process SHALL exit only on `end`/EOF. `--include-partial-messages` SHALL add `text_delta` and `reasoning_delta` events; `--include-hook-events` SHALL add `hook_event` lines as "Hook events in exec" defines; `--output-last-message <file>` SHALL write the final assistant text of the last Turn to that file. All added types SHALL be declared in the `init` event's `schema_version` 2.

#### Scenario: SDK-style multi-turn driver
- **WHEN** a program starts `cyber exec --input-format stream-json --format stream-json` and writes two `user` lines followed by `end`
- **THEN** stdout carries two `result` events in order and the process exits 0 after the second Drain goes idle

#### Scenario: Permission answered over stdin
- **WHEN** the model needs approval for `bash` and the driver writes a `permission_reply` with `reply: "once"`
- **THEN** the command runs and no `permission_denied` event is emitted

## ADDED Requirements

### Requirement: Hook events in exec
(P1) `cyber exec --include-hook-events` SHALL add one `hook_event` line to `--format stream-json` output, in single-prompt and `--input-format stream-json` runs, for each hook execution of the run's Session once its receipt settles. A line SHALL carry `receipt_id`, `hook_id` (the configured ID, else the definition digest), `event`, `scope`, `outcome` (`ok`, `blocked`, `error`, `timeout`, `skipped`), `decision` (the decision kind or null), `duration_ms` and `started_ms`, plus `tool_name` and `call_id` when the receipt has them, and the usual `ts` and `session_id`. `hook_id` and `tool_name` SHALL pass through credential redaction. A line SHALL NOT carry hook stdin, stdout, stderr, the decision reason, rewritten input, added context or environment, even when `telemetry.log_hook_io` records IO on the receipt.

Lines SHALL follow the live receipt summaries (`hooks` → Hook receipt summaries for clients), so the line of a synchronous hook precedes the line of the action it gated (a `PreToolUse` line before that call's `tool_result`, a `Stop` line before `result`). Before each `result` line, before `end`, and when it subscribes to a Session it created, exec SHALL reconcile with the durable receipt listing and add the settled receipts that have no line yet, oldest first, so a receipt the live stream missed is still reported exactly once. For a reused Session (`--session`, `--continue` without `--fork`) only receipts admitted after the run attached SHALL be reported. Receipts of other Sessions (subagents, delegated `@agent` prompts, Workflow agents) and receipts still running when the run ends SHALL get no line.

Without the flag the output SHALL be unchanged. With any other `--format`, or with `--workflow`, exec SHALL exit 2 before any Session exists. The line is an opt-in addition and SHALL NOT change the `init` line's `schema_version`; `hook_event` is therefore exempt from the rule that added types are declared by `schema_version` 2, because a reader only receives it after asking for it.

#### Scenario: A blocking guard in CI
- **WHEN** `cyber exec --format stream-json --include-hook-events "run the tests"` calls `bash` and a `PreToolUse` command hook `guard` exits 2 for it
- **THEN** a `hook_event` line with `hook_id: "guard"`, `event: "PreToolUse"`, `outcome: "blocked"`, `tool_name: "bash"` and that call's `call_id` appears before the call's `tool_result` line, and the hook's stderr is not in the line

#### Scenario: Hook IO stays out
- **WHEN** a hook prints a token from its environment to stderr and into its decision reason, with `telemetry.log_hook_io` on
- **THEN** the token is in the durable receipt but on no stdout line

#### Scenario: Wrong format
- **WHEN** a user runs `cyber exec --format json --include-hook-events "hi"`
- **THEN** exec exits 2 with `--include-hook-events needs --format stream-json` and no Session is created

#### Scenario: Reused Session
- **WHEN** a second run uses `--session ses_1 --include-hook-events` after an earlier run of `ses_1` ran hooks
- **THEN** only the hooks of the second run get lines
