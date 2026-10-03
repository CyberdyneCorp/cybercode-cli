# exec-mode Specification

## Purpose
`cyber exec` runs Cyber Code non-interactively for scripts, CI and other programs. It sends one prompt, slash command, goal or workflow to a Session, streams results as text or machine-readable JSONL, and exits with a well-defined code. It draws from OpenCode's `run` command (message assembly, JSON events, attach), Codex's `exec` (output schema, ephemeral runs), and Claude Code's headless `-p` mode (stream-json, max turns, permission modes), extended with goal and workflow entry points.

## Requirements

### Requirement: Command and prompt assembly
(P0) The system SHALL provide `cyber exec [prompt...]` with the global alias `cyber -p [prompt...]`. Positional words SHALL be joined with spaces. When stdin is not a TTY, all of stdin SHALL be read and appended after the prompt, separated by a blank line, or used alone when no prompt is given. An empty prompt without `--command`, `--goal` or `--workflow` SHALL fail with exit code 2.

#### Scenario: Stdin plus prompt
- **WHEN** a user runs `cat error.log | cyber exec "explain the root cause"`
- **THEN** the submitted message is `explain the root cause`, a blank line, then the log content

#### Scenario: Empty input rejected
- **WHEN** a user runs `cyber exec` with a TTY stdin and no arguments
- **THEN** the command prints `exec requires a prompt, --command, --goal or --workflow` and exits with code 2

### Requirement: Execution backend and working directory
(P0) The system SHALL run against the registered background server by default, starting it as `cyber service start` does. `--embedded` SHALL run a private in-process server on a private database (in-memory when combined with `--ephemeral`) that never opens the shared database for writing. `--attach <url>` SHALL target a remote server. Remote authentication SHALL use `--password` or `CYBER_SERVER_PASSWORD`, or the Cyber Account token for Relay URLs. The global `--cwd <dir>` SHALL set the Location before execution and fail with exit code 2 when the directory does not exist locally, unless `--attach` is set, in which case the path is sent to the server as-is.

#### Scenario: Attach to a remote runner
- **WHEN** CI runs `cyber exec --attach https://runner.internal:4747 --cwd /work/repo "run lint"`
- **THEN** the Session is created on the remote server in `/work/repo` and events stream back to the CI log

#### Scenario: Default uses the background server
- **WHEN** a user runs `cyber exec "hi"` while the TUI has a Session running on the registered server
- **THEN** the prompt is admitted through that server and no second writer process is started

### Requirement: Session selection
(P0) The system SHALL create a new Session by default. `--continue` SHALL reuse the most recent root Session of the Location. `--session <id|name>` SHALL reuse that Session or fail with `Session not found` and exit code 1. `--fork` SHALL fork the selected Session first and SHALL require `--continue` or `--session`. `--name <name>` SHALL name the new Session.

#### Scenario: Resume by name
- **WHEN** a nightly job runs `cyber exec --session nightly-triage "continue triage"`
- **THEN** the prompt is admitted to the existing `nightly-triage` Session with its prior history

### Requirement: Ephemeral runs
(P0) The system SHALL accept `--ephemeral`, which implies `--embedded` and runs the Session in an in-memory store. Nothing SHALL be written to the shared database, snapshots or share service, and no resume hint SHALL be printed. `--ephemeral` combined with `--continue`, `--session` or `--fork` SHALL fail with exit code 2.

#### Scenario: Nothing persisted
- **WHEN** `cyber exec --ephemeral "summarize README"` completes
- **THEN** `cyber sessions list` shows no new Session

### Requirement: Model, agent and mode selection
(P0) The system SHALL accept `--model provider/model[#variant]`, `--agent <name>` and `--mode <mode>`. The default mode for exec Sessions SHALL be `dont-ask`: actions allowed by rules run, and anything that would prompt is denied and logged. `--auto` SHALL select `auto` mode. `--yolo` SHALL select `bypass` and print a warning to stderr, and it SHALL be refused when org policy disables bypass. An unknown agent or a subagent-only agent SHALL fail with exit code 2.

#### Scenario: Default dont-ask
- **WHEN** an exec Session with no mode flag hits an `edit` that would require `ask`
- **THEN** the edit is denied without blocking, a `permission_denied` event is emitted, and the model receives the denial as a tool error

#### Scenario: Bypass disabled by policy
- **WHEN** org policy disables bypass and a user runs `cyber exec --yolo "deploy"`
- **THEN** the command prints `bypass mode is disabled by your organization` and exits with code 2

### Requirement: Never blocking on interaction
(P0) Exec Sessions SHALL carry Session rules that deny the `question` tool, `plan_enter` and `plan_exit`, so the agent can never wait for a human answer. These rules SHALL NOT be added to existing Sessions reused with `--session` or `--continue` unless `--non-interactive-rules` is passed.

#### Scenario: Question tool unavailable
- **WHEN** an exec Session's agent tries to call `question`
- **THEN** the tool is not offered to the model and no pending question is ever created

### Requirement: Permission denial log
(P1) The system SHALL record every permission denial during an exec run with tool, action, resources and reason (`rule`, `mode`, `sandbox`, or `policy`). The denials SHALL be included in the final `result` event, and printed to stderr as `denied: <tool> <resource> (<reason>)` in text mode. `--fail-on-deny` SHALL make any denial end the run with exit code 5.

#### Scenario: Fail on deny in CI
- **WHEN** `cyber exec --fail-on-deny "fix lint"` is denied `bash "curl https://example.com"` by the sandbox
- **THEN** the run stops after the current Turn, prints `denied: bash curl https://example.com (sandbox)`, and exits with code 5

### Requirement: Slash commands, files and attachments
(P0) The system SHALL accept `--command <name>`, which runs a custom command or skill with the assembled prompt as its arguments. `--file <path>` (repeatable) SHALL attach files resolved against the Location and fail with exit code 2 for missing paths. With `--attach`, regular files up to 10 MiB SHALL be embedded as data URLs, and directories SHALL be rejected.

#### Scenario: Run a custom command
- **WHEN** a user runs `cyber exec --command review "main..HEAD"`
- **THEN** the `review` command template is expanded with `$ARGUMENTS` = `main..HEAD` and executed

### Requirement: Output formats
(P0) The system SHALL honor the global `--format text|json|stream-json` (default `text` for `exec`):
- **text**: final assistant text on stdout; tool progress lines on stderr only when `--verbose` is set.
- **json**: one JSON object on stdout at the end, with `result`, `session_id`, `usage`, `cost_usd`, `duration_ms`, `num_turns`, `denials` and `exit_code`.
- **stream-json**: one JSON object per line as events occur.

`--quiet` SHALL suppress all non-result output on stderr.

#### Scenario: JSON result for scripting
- **WHEN** a user runs `cyber exec --format json "count TODOs"`
- **THEN** stdout contains exactly one JSON object whose `result` holds the final text and whose `exit_code` is 0

### Requirement: Stream JSON event schema
(P0) In `stream-json` mode, each line SHALL be an object with `type`, `ts` (epoch ms) and `session_id`. The types are:

| type | Payload | When |
|---|---|---|
| `system` (`subtype: "init"`) | model, agent, mode, cwd, tool names, MCP server statuses | first |
| `assistant_text` | message ID, text | each finished text part |
| `reasoning` | reasoning text | finished reasoning parts, only with `--thinking` |
| `tool_use` | call ID, tool, input | — |
| `tool_result` | call ID, status, output (truncated to 2,000 chars) | — |
| `permission_denied` | — | each denial |
| `goal_verdict` | — | goal-driven runs |
| `workflow_progress` | — | workflow runs |
| `error` | — | — |
| `result` | same shape as the json-mode result | last |

The schema SHALL be versioned by `schema_version` in the `init` event (starting at `1`), and changes SHALL be additive within a major version.

#### Scenario: Event order
- **WHEN** a stream-json run makes one bash call and finishes
- **THEN** stdout lines are, in order: `system`, `tool_use`, `tool_result`, `assistant_text`, `result`

### Requirement: Structured final output
(P1) The system SHALL accept `--output-schema <file.json>`, a JSON Schema the final answer must satisfy. It SHALL be enforced through the structured-output mechanism from `session-runtime`. On success, the `result` field SHALL hold the validated JSON object (text mode prints it pretty-printed). After 2 failed validation retries the run SHALL exit with code 1 and a `StructuredOutputError`.

#### Scenario: Valid structured result
- **WHEN** a user runs `cyber exec --output-schema findings.schema.json "audit deps"` and the model returns a conforming object
- **THEN** stdout is that JSON object and the exit code is 0

### Requirement: Run limits
(P0) The system SHALL accept the Budget flags defined in `observability-costs`:
- `--max-turns <n>`: on reaching the limit, the final Turn is sent without tools.
- `--max-tokens <n>` and `--max-cost <usd>`: checked after each Turn.
- `--timeout <duration>`, for example `30m`.

Exceeding `--max-tokens`, `--max-cost` or `--timeout` SHALL interrupt the Drain, emit a `result` with `stop_reason` `budget_exceeded` or `timeout`, and exit with code 4.

#### Scenario: Cost cap reached
- **WHEN** a run with `--max-cost 0.50` reaches $0.53 after a Turn
- **THEN** the Drain is interrupted, `result.stop_reason` is `budget_exceeded`, and the exit code is 4

### Requirement: Goal-driven exec
(P2) The system SHALL accept `--goal "<condition>"`, which sets the condition as the Session's active Goal (see `goals`) and keeps running until the evaluator settles. Exit codes: 0 when the goal is met, 3 when the evaluator judges it impossible, 4 when the goal or run budget is exhausted. Each verdict SHALL be emitted as a `goal_verdict` event. A positional prompt, when given, SHALL be sent as the first message.

#### Scenario: Goal met
- **WHEN** CI runs `cyber exec --goal "cargo test passes" "fix the failing tests"` and the evaluator returns `met`
- **THEN** the run exits with code 0 and the final `result` includes `goal: { status: "met", reason }`

#### Scenario: Goal impossible
- **WHEN** the evaluator returns `impossible: requires credentials not available in CI`
- **THEN** the run exits with code 3 and prints the reason to stderr

### Requirement: Workflow exec
(P2) The system SHALL accept `--workflow <name|path>` with `--args <json>`, which starts a Workflow Run as defined in `workflows`, streams `workflow_progress` events, and prints the run's return value as the result. A workflow that throws SHALL exit with code 1. A run stopped by its budget SHALL exit with code 4.

#### Scenario: Run a saved workflow
- **WHEN** a user runs `cyber exec --workflow audit-deps --args '{"severity":"high"}'`
- **THEN** the Workflow Run starts with that input, progress lines stream to stderr, and the returned JSON is written to stdout

### Requirement: Exit codes
(P0) The system SHALL use these exit codes:

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | runtime error (provider, tool or session error) |
| 2 | usage or configuration error |
| 3 | goal judged impossible |
| 4 | budget or timeout exceeded |
| 5 | permission denied with `--fail-on-deny`, or the run cannot proceed without approval |
| 6 | Cyber Account or entitlement required |
| 130 | interrupted by SIGINT |

SIGINT SHALL interrupt the Drain gracefully, flush the `result` event, and then exit.

#### Scenario: Ctrl+C in CI
- **WHEN** the process receives SIGINT mid-Turn
- **THEN** unsettled tools are marked interrupted, a `result` with `stop_reason: "interrupted"` is emitted, and the exit code is 130

### Requirement: CI usage
(P1) The system SHALL run headless in CI without a TTY, config files or account. It SHALL read provider credentials from environment variables and never open a browser. Output SHALL contain no ANSI color codes unless `--color always` is set. The resume hint SHALL be printed to stderr as `session: <id>` only when the Session is persisted.

#### Scenario: GitHub Actions step
- **WHEN** a workflow step runs `cyber exec --format stream-json --max-turns 30 "fix lint errors"` with `OPENAI_API_KEY` set
- **THEN** the run completes without prompts, logs JSONL to stdout, and the job fails only when the exit code is non-zero

#### Scenario: GitLab CI job
- **WHEN** a `.gitlab-ci.yml` job runs `cyber exec --mode accept-edits "update snapshots" && git diff --exit-code || true`
- **THEN** edits apply without approval prompts and no color codes appear in the job log

### Requirement: Session sharing from exec
(P3) The system SHALL accept `--share`, which shares the Session after creation as defined in `session-sharing` and prints `share: <url>` to stderr. When sharing is disabled by config or policy, it SHALL print a warning without failing the run.

#### Scenario: Share disabled
- **WHEN** a user runs `cyber exec --share "explain"` with `share: "disabled"`
- **THEN** stderr shows `sharing is disabled` and the run continues normally

### Requirement: Streaming input and partial output
(P1) `--input-format stream-json` SHALL read JSON lines from stdin for the life of the process: `{ "type": "user", "text" | "parts", "delivery"? }` admits a prompt, `{ "type": "interrupt" }` interrupts the Drain, `{ "type": "permission_reply", "request_id", "reply", "message"? }` and `{ "type": "question_reply", "request_id", "answers" }` answer pending requests (which are emitted as `permission_request` and `question` events instead of being auto-denied), and `{ "type": "end" }` or EOF ends the run after the current Drain goes idle. Each prompt SHALL produce its own `result` event and the process SHALL exit only on `end`/EOF. `--include-partial-messages` SHALL add `text_delta` and `reasoning_delta` events; `--include-hook-events` SHALL add `hook_event` lines (hook id, event, outcome, decision); `--output-last-message <file>` SHALL write the final assistant text of the last Turn to that file. All added types SHALL be declared in the `init` event's `schema_version` 2.

#### Scenario: SDK-style multi-turn driver
- **WHEN** a program starts `cyber exec --input-format stream-json --format stream-json` and writes two `user` lines followed by `end`
- **THEN** stdout carries two `result` events in order and the process exits 0 after the second Drain goes idle

#### Scenario: Permission answered over stdin
- **WHEN** the model needs approval for `bash` and the driver writes a `permission_reply` with `reply: "once"`
- **THEN** the command runs and no `permission_denied` event is emitted
