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
(P0) The system SHALL accept `--model provider/model[#variant]`, `--agent <name>`, `--mode <mode>` and `--output-style <name>`. The default mode for exec Sessions SHALL be `dont-ask`: actions allowed by rules run, and anything that would prompt is denied and logged. `--auto` SHALL select `auto` mode. `--yolo` SHALL select `bypass` and print a warning to stderr, and it SHALL be refused when org policy disables bypass. `--dangerously-skip-permissions` SHALL select `bypass` together with the override of `--dangerously-bypass-permissions` and print a warning to stderr; like that flag it SHALL require `--embedded` or `--ephemeral`, failing otherwise with exit code 2 and a message naming them, and it SHALL fail with exit code 2 when combined with `--auto` or a `--mode` other than `bypass`. It SHALL be refused when org policy disables bypass. An unknown agent or a subagent-only agent SHALL fail with exit code 2. An invalid, unknown or held `--output-style` SHALL fail with exit code 2 and the reason, before a Session is created. Without the flag, a configured output style that is unavailable SHALL produce a stderr warning naming it and the reason, and the run SHALL use `default`.

#### Scenario: Default dont-ask
- **WHEN** an exec Session with no mode flag hits an `edit` that would require `ask`
- **THEN** the edit is denied without blocking, a `permission_denied` event is emitted, and the model receives the denial as a tool error

#### Scenario: Bypass disabled by policy
- **WHEN** org policy disables bypass and a user runs `cyber exec --yolo "deploy"`
- **THEN** the command prints `bypass mode is disabled by your organization` and exits with code 2

#### Scenario: Claude Code's skip flag
- **WHEN** a user runs `cyber exec --embedded --dangerously-skip-permissions "fix it"` on a host with no sandbox or container
- **THEN** the Session runs in `bypass`

#### Scenario: Skip flag on the shared server
- **WHEN** a user runs `cyber exec --dangerously-skip-permissions "fix it"` without `--embedded` or `--ephemeral`
- **THEN** the command prints `--dangerously-skip-permissions needs --embedded or --ephemeral` and exits with code 2 without creating a Session

#### Scenario: Output style flag
- **WHEN** a user runs `cyber exec --output-style concise "explain the build"`
- **THEN** the Session's system prompt ends with the `concise` style

#### Scenario: Unknown output style
- **WHEN** a user runs `cyber exec --output-style nope "hi"`
- **THEN** the command prints that output style `nope` is not defined, lists the available names, and exits with code 2 without creating a Session

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
(P0) The system SHALL accept `--command <name>`, which runs a custom command or skill with the assembled prompt as its arguments. `--file <path>` (repeatable) SHALL attach files resolved against the Location and fail with exit code 2 for missing paths. Directories SHALL be rejected. Text files SHALL be attached as text. Images (PNG, JPEG, GIF, WebP) and PDFs, detected from their bytes, SHALL be attached as attachment parts (`server-api` → Prompt attachment parts):

- **Local server:** as `file` parts, which the server reads under the `read` permission and the exec mode. A path outside the Location in `dont-ask` mode is denied unless `--add-dir` covers it.
- **`--attach <url>`:** as inline bytes read by the CLI.

Attachments SHALL obey the server's `attachments.max_bytes` (default 20 MiB). A refused attachment (too large, unsupported type, model without the input, or a permission denial) SHALL end the run with exit code 2 and the server's message, before any Turn runs.

#### Scenario: Run a custom command
- **WHEN** a user runs `cyber exec --command review "main..HEAD"`
- **THEN** the `review` command template is expanded with `$ARGUMENTS` = `main..HEAD` and executed

#### Scenario: Screenshot attached
- **WHEN** a user runs `cyber exec --file shot.png "what is wrong in this UI?"` with a vision model
- **THEN** the model receives the image and the run exits with code 0

#### Scenario: Text-only model
- **WHEN** the same command runs with a model that has no image input
- **THEN** the run exits with code 2 and stderr names the model and `has no image input`

### Requirement: Output formats
(P0) The system SHALL honor the global `--format text|json|stream-json` (default `text` for `exec`):
- **text**: final assistant text on stdout; tool progress lines on stderr only when `--verbose` is set.
- **json**: one JSON object on stdout at the end, with `result`, `session_id`, `usage`, `cost_usd`, `duration_ms`, `num_turns`, `denials` and `exit_code`.
- **stream-json**: one JSON object per line as events occur.

`--quiet` SHALL suppress all non-result output on stderr.

#### Scenario: JSON result for scripting
- **WHEN** a user runs `cyber exec --format json "count TODOs"`
- **THEN** stdout contains exactly one JSON object whose `result` holds the final text and whose `exit_code` is 0

### Requirement: Background work owned by the run
(P1) A run SHALL NOT finish while background work it started is unsettled. That work is background subagent Jobs and forked skill commands, including a forked command that was forwarded but has no Job yet. After the Session goes idle, exec SHALL keep following it until every Job the run started has settled, its handback has been admitted, and the Turn that handles the handback has gone idle. `result` SHALL be the final assistant text of that last Turn, and usage SHALL include descendant spend. Background tasks that already existed on a reused Session are not owned by the run and SHALL NOT be awaited or cancelled. A forked command whose admission outcome is unknown SHALL end the run with an error that names the delegation to inspect. Owned work that ended but whose handback was not admitted (the server reported a `background_settlement` error, or no handback arrived within a bounded grace period) SHALL end the run with an error that names the Job or delegation to inspect instead of waiting indefinitely. When `--timeout`, `--max-cost`, `--max-tokens` or SIGINT stops a run, exec SHALL cancel only the background work it owns, then interrupt the Drain so no handback Turn runs after exit, and report the usual `stop_reason` and exit code. Owned work the Drain started before it acknowledged the interrupt SHALL also be cancelled before the run ends. `cyber review` (`cli-commands`) SHALL use the same owned-work rules as `cyber exec --command review`: it waits for the reviewer Job and its handback Turn, cancels only that work on timeout, budget or SIGINT, and reports the Job's validated structured result.

#### Scenario: Forked command answer
- **WHEN** a user runs `cyber exec --command audit --format json` and `audit` declares `context: fork`
- **THEN** exec exits after the forked child completes and the parent's handback Turn goes idle, with that Turn's final text as `result` and `exit_code` 0

#### Scenario: Timeout while a child runs
- **WHEN** `--timeout 30s` elapses while the parent is idle and an owned background Job is still running
- **THEN** the Job is cancelled, no handback Turn runs after exit, `stop_reason` is `timeout` and the exit code is 4

#### Scenario: Handback admission fails
- **WHEN** an owned background Job ends but the server reports that its handback could not be admitted
- **THEN** exec exits with code 1 and an `error` naming the Job instead of waiting for a handback that will never run

### Requirement: Stream JSON event schema
(P0) In `stream-json` mode, each line SHALL be an object with `type`, `ts` (epoch ms) and `session_id`. The types are:

| type | Payload | When |
|---|---|---|
| `system` (`subtype: "init"`) | model, agent, mode, cwd, tool names, MCP server statuses | first |
| `assistant_text` | message ID, text | each finished text part |
| `reasoning` | reasoning text | finished reasoning parts, only with `--thinking` |
| `tool_use` | call ID, tool, input | — |
| `tool_result` | call ID, status, output (truncated to 2,000 chars) | — |
| `retry` | message ID, attempt, delay_ms, error, discarded | each provider retry |
| `permission_denied` | — | each denial |
| `goal_verdict` | — | goal-driven runs |
| `workflow_progress` | — | workflow runs |
| `error` | — | — |
| `result` | same shape as the json-mode result | last |

The schema SHALL be versioned by `schema_version` in the `init` event (starting at `1`), and changes SHALL be additive within a major version. Only recorded content SHALL produce `assistant_text`, `reasoning` and `tool_use` lines, so an attempt the retry policy discarded never appears.

#### Scenario: Event order
- **WHEN** a stream-json run makes one bash call and finishes
- **THEN** stdout lines are, in order: `system`, `tool_use`, `tool_result`, `assistant_text`, `result`

#### Scenario: Answer replayed after a dropped connection
- **WHEN** the provider connection drops after part of the answer and the replayed request completes
- **THEN** stdout has one `retry` line with `discarded: true`, then a single `assistant_text` holding only the replayed answer

### Requirement: Structured final output
(P1) The system SHALL accept `--output-schema <file.json>`, a JSON Schema the final answer must satisfy. It SHALL be enforced through the structured-output mechanism from `session-runtime`. On success, the `result` field SHALL hold the validated JSON object (text mode prints it pretty-printed). After one initial submission and at most two failed validation corrections the run SHALL exit with code 1 and a `StructuredOutputError`; it SHALL NOT make a fourth submission.

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

### Requirement: Usage limit waits in exec
(P1) `cyber exec` SHALL create its Sessions with `unattended: true`, so a usage limit with a known reset waits as `session-runtime` → Usage limit wait defines. In `stream-json` mode each wait change SHALL be one line of type `usage_limit` with `status` (`waiting`, `resumed` or `abandoned`), `message_id`, and for `waiting` `resets_at_ms` and `resume_at_ms`, for `abandoned` `reason`. This type is an additive change within `schema_version` 1. In text mode, unless `--quiet`, exec SHALL print one stderr line when a wait starts, naming the local reset and resume times, and one when it resumes. When `--timeout` would expire before the planned resume time, exec SHALL stop the run at once with `stop_reason` `timeout` and exit code 4.

#### Scenario: Stream of a waited run
- **WHEN** a stream-json run hits a usage limit that resets in 2 minutes and then succeeds
- **THEN** stdout has a `usage_limit` line with `status: "waiting"`, then one with `status: "resumed"`, then `assistant_text` and a `result` with exit code 0

#### Scenario: Timeout before the reset
- **WHEN** `cyber exec --timeout 10m` hits a usage limit that resets in 40 minutes
- **THEN** exec stops the run right after the `waiting` event with `stop_reason: "timeout"` and exit code 4

### Requirement: Redaction notes in exec output
(P1) A `tool_result` line of `--format stream-json` SHALL carry the settlement's `redactions` when it hid anything (`tool-registry` → Redaction visibility). In text mode without `--quiet`, `cyber exec` SHALL print `<tool>: <n> secret(s) hidden from the model` to stderr.

#### Scenario: Stream line
- **WHEN** a `bash` call settles with two values hidden
- **THEN** its `tool_result` line has `"redactions": {"count": 2, "kinds": […]}`

### Requirement: Refused and truncated answers in exec
(P1) When a run ends on its own and its last finished step stopped with `content_filter`, the result's `stop_reason` SHALL be `refusal`; with `length`, `max_tokens`. Other runs keep `end_turn` or `error`, and a stop reason set by a limit, timeout or interrupt takes precedence. In text mode, unless `--quiet`, exec SHALL print one `warning:` line on stderr for each step that stopped this way. The exit code is unchanged.

#### Scenario: Refusal in exec
- **WHEN** `cyber exec --format json` runs a prompt the model refuses with `content_filter`
- **THEN** `result` holds the refusal text, `stop_reason` is `refusal` and the exit code is 0

### Requirement: Bounded interactive JSONL driver
(P1) `--input-format stream-json` SHALL parse bounded typed user/control/reply/end envelopes independently of model execution, retain durable admission identity and emit one correlated terminal result per admitted input under schema version 2. Replies SHALL target only live requests owned by that driver. EOF/end SHALL stop new admission and drain acknowledged owned work without leaving unattended asks pending indefinitely or cancelling unrelated Session work. Malformed, oversized and over-capacity frames SHALL produce explicit protocol errors rather than silent text admission. `--output-schema` SHALL use root structured results; unstructured stdin behavior SHALL remain compatible.

#### Scenario: Direct model requests retain their complete input cohort
- **WHEN** a model step starts on the active branch
- **THEN** its server-recorded cohort SHALL contain every input driving that step, preserve it across tool continuations and compaction, and exclude inactive branch promotions
- **AND** runtime append and replay SHALL refuse contradictory nonempty provenance; legacy or oversized cohorts SHALL grant no inferred ownership
- **WHEN** the step's exact tool call opens a permission or question request
- **THEN** the pending request SHALL expose its captured `input_message_ids`, bounded to 32 identities of at most 256 UTF-8 bytes each

#### Scenario: An admission acknowledgement is lost
- **WHEN** a prompt submission and its single resubmission under the same message identity both lose their response
- **THEN** the driver SHALL read the Session inbox for that message identity before reporting the input
- **AND** a held row that is not refused SHALL report the input `admitted` and track it to its result; no row SHALL report it `refused`
- **AND** when the inbox cannot be read the run SHALL end with a fatal error (exit 2) naming the message identity rather than report an input the server may run as refused

#### Scenario: Two drivers have pending input in one Session
- **GIVEN** the first driver owns a live direct request and the second driver has admitted a different input
- **WHEN** the second driver sees that request, tries to reply, or closes while the first driver remains attached
- **THEN** it SHALL neither retain the request as its own nor reply to or dismiss it
- **AND** the first driver SHALL retain normal reply authority, while a closed driver SHALL decline only its own later direct requests
- **AND** user-provided correlation labels SHALL NOT grant request ownership

#### Scenario: Only the bound input client can keep an owned ask waiting
- **WHEN** an input-client lease admits an original root user input
- **THEN** its immutable binding SHALL commit atomically with admission, participate in exact retry identity, and match that admission's digest
- **WHEN** its complete recorded input cohort opens a direct request, including through an authenticated Job handback
- **THEN** only one live input client bound to that complete cohort and Location SHALL keep it waiting
- **WHEN** that client releases or expires while another client, browser or stream remains present
- **THEN** its requests SHALL settle unattended without borrowing other attendance, and the other client's requests SHALL remain independent
- **AND** scoped input leases SHALL grant neither Session-wide attendance nor native computer authority; malformed, missing scoped evidence and mixed bindings SHALL NOT fall back to legacy attendance

#### Scenario: A descendant request carries its root input cohort
- **WHEN** a child Session asks for permission or a question and every link from it up to its root Session is proven, either by a running background subagent Job whose immutable input origin is complete, bounded (1–32 unique identities of at most 256 UTF-8 bytes) and identical across that link's running Jobs, or by the parent's foreground (synchronous) subagent call that is running in this server process and whose exact dispatched call has such a complete recorded input cohort
- **AND** when a running Job and a running foreground call both prove one link, their cohorts are identical
- **THEN** the durable and routed request SHALL expose `root_session_id` and that root link's `root_input_message_ids`
- **AND** when the root cohort is bound to an input client, only that live client at the bound Session and Location SHALL keep the request waiting and have its reply accepted; its release or expiry SHALL settle the request unattended without borrowing browser, stream or other client attendance
- **WHEN** a link is missing (a detached child, one whose background Job is no longer running, or one whose foreground call has returned), mixed, malformed, oversized, cyclic or deeper than 32 links
- **THEN** the request SHALL carry no root cohort and SHALL keep its existing attendance; it SHALL grant no inferred input ownership
- **WHEN** the server restarts
- **THEN** the root cohort SHALL remain derivable from durable Job provenance and recorded requests, but SHALL NOT recreate the bound client's live attendance, and neither the interrupted Job nor the foreground call that the restart ended SHALL stamp new requests

#### Scenario: The driver owns descendant and hook-originated requests
- **WHEN** a request routed to the driven Session carries `root_session_id` equal to that Session and every `root_input_message_ids` identity is a pending input of the driver
- **THEN** the driver SHALL emit it with the asking child Session as `request_session_id`, accept a reply only for that request kind, and post the reply to the child Session
- **AND** the owning inputs SHALL stay pending while the request waits; EOF/end SHALL settle it (lease release, then decline) instead of leaving it pending
- **WHEN** another driver of the same Session sees that request
- **THEN** it SHALL neither emit nor answer it, and its reply SHALL be a protocol error
- **WHEN** a `PreToolUse` hook decision of `ask` opens a permission request
- **THEN** that request SHALL carry its step's input cohort and be owned like a direct request
- **WHEN** a tracked request is answered elsewhere
- **THEN** the driver SHALL stop tracking it

#### Scenario: Input presence survives as provenance rather than live authority
- **WHEN** an already admitted input is retried after expiry or restart
- **THEN** the original admission receipt SHALL remain retrievable without renewing its lease, and a different client under that input identity SHALL conflict
- **AND** replayed binding metadata SHALL NOT recreate live attendance; legacy admissions without scoped negotiation SHALL preserve their existing behavior
- **WHEN** the driver observes its Session events
- **THEN** its connection SHALL explicitly opt out of broad Session presence while preserving replay/live cursor behavior and ordinary stream defaults

#### Scenario: Replies cannot cross input clients or consume a different request type
- **WHEN** an input client's request is answered
- **THEN** a scoped reply SHALL select its waiter under the live attendance check, and reject/always cascades SHALL NOT select another input client's pending requests
- **AND** a stop reply SHALL signal only the exact Drain whose complete current cohort belongs to that request, while legacy behavior remains compatible
- **WHEN** a question reply names a permission request or conversely
- **THEN** the API SHALL refuse without consuming the original request or stopping its work

#### Scenario: Interrupt while inference runs
- **WHEN** an active JSONL driver sends an interrupt while its model request is streaming
- **THEN** the reader processes it without waiting for model completion and interruption applies only to the driver's owned execution

#### Scenario: Retained interruption cannot target a replacement Drain
- **WHEN** an interruption selects 1–32 unique original input identities and a stable request identity
- **THEN** its intent and queued refusals SHALL commit before signalling the exact captured Drain, whose complete input cohort must belong to the selection
- **AND** mixed or unavailable provenance SHALL NOT authorize cancellation of that Drain or withdrawal of its requests
- **WHEN** the requester disappears or retries after another Drain starts
- **THEN** cleanup SHALL retain its runtime shutdown owner, wait for the captured owner's actual disposal, and return the original receipt without selecting replacement execution
- **AND** a different cohort under the same request identity SHALL refuse; missing retained cleanup or a changed runtime generation SHALL report unknown reconciliation rather than inventing acknowledgement

#### Scenario: Interruption acknowledgement is ahead of client replay
- **WHEN** the server has acknowledged the exact retained interruption but its receipt revision is ahead of the driver's consumed durable events
- **THEN** the driver SHALL retain the selected input slots until replay reaches that revision
- **AND** polling SHALL leave stdin and event processing live; a newer input and its request reply authority SHALL remain independent
- **WHEN** a submission response is lost
- **THEN** lookup and any necessary resubmission SHALL use the same request identity and original cohort, without a Session-wide fallback

#### Scenario: An interrupted input has direct process Jobs
- **WHEN** the captured Drain has released and pending Job publication has crossed its admission barrier
- **THEN** cancellation SHALL select only proven direct process Jobs of the exact Session and complete selected cohort, in bounded identity pages
- **AND** detached, mixed and descendant work SHALL NOT gain cancellation authority from Session membership; unresolved selected scope SHALL report unknown
- **WHEN** a completion notifier races the interruption intent
- **THEN** its exact authenticated handback SHALL remain refused and SHALL NOT restart the interrupted input, including after receipt settlement or restart
- **AND** these control receipts SHALL NOT acknowledge native effects or erase ordinary recovery evidence

#### Scenario: An interrupted input owns a background subagent
- **WHEN** a running subagent Job of the exact Session has a complete durable input cohort within the selection, its child is a direct child of that Session, and no Job from outside the child's bounded subtree targets any Session in it
- **THEN** the interruption SHALL suppress that Job's handback and stop the child subtree through the subtree stop path, counting the Job only when that stop is acknowledged
- **AND** a mixed, detached, oversized or otherwise unproven descendant SHALL remain running and the receipt SHALL report `descendant_job` with status unknown
- **AND** a subtree stop that leaves unknown execution, such as a tool call cancelled mid-flight, SHALL report unknown and keep the child's admission closed for recovery
- **WHEN** a subtree tool call is still held at its permission or question request
- **THEN** the stop SHALL withdraw that request before cancelling the call, the call SHALL settle as refused without running, and that call SHALL NOT make the stop unknown
- **WHEN** the selected Drain is running a foreground subagent call
- **THEN** cancelling that Drain SHALL stop the child with the call, and the call SHALL settle as outcome unknown, because a call cancelled while its child ran cannot itself prove the child's effects; the input SHALL fail while later input stays admissible

#### Scenario: Bounded background ownership lookup
- **WHEN** an authenticated client queries Job state for 1–32 unique input identities, each at most 256 UTF-8 bytes
- **THEN** the server SHALL return one small record per supplied identity without returning Job payloads or collecting Session history pages
- **AND** a Job SHALL count only when its complete valid durable input cohort belongs to that Session and the supplied set; missing, malformed, oversized, duplicate or mixed-cohort provenance SHALL grant no inferred ownership
- **AND** the lookup SHALL NOT mutate, cancel or settle work, and handback admission SHALL NOT be treated as proof that its parent Turn has completed

#### Scenario: A handback starts a request or another Job
- **WHEN** a Job's handback is admitted
- **THEN** its notifier acceptance SHALL commit with that input before inference starts, and a rejected notifier write SHALL publish neither the input nor inference
- **WHEN** the handback-driven step opens a request or starts another Job
- **THEN** its input provenance SHALL preserve the complete original input cohort and any new steering inputs through exact durable admission/notification/Job-origin evidence preceding that step
- **AND** a source label, synthetic name, later notification or contradictory Job origin SHALL NOT grant inferred ancestry; unknown or excessive cohorts SHALL NOT become partial ownership

#### Scenario: Background work outlives the initial answer
- **WHEN** the initial answer ends while its input still owns running background work or an admitted handback with unfinished model/tool work
- **THEN** the driver SHALL retain that input's pending slot and SHALL NOT emit its terminal result
- **WHEN** the handback ends and the driver has consumed the durable revision observed by the bounded Job query
- **THEN** the driver SHALL emit the correlated final handback answer and release that slot
- **AND** completion of one driver's input SHALL NOT wait for another driver's active question or adopt its text

#### Scenario: Replay and live notices share a driver connection
- **WHEN** the driver opens a Session event stream with a durable cursor and `live=true`
- **THEN** it SHALL receive contiguous durable replay plus live idle/delta notices for that Session on the same owned connection
- **AND** live notices SHALL NOT advance the durable SSE cursor; omitting the option SHALL preserve the existing stream behavior

#### Scenario: The server restarts under an attached driver
- **GIVEN** a driver attached to a server with an admitted input
- **WHEN** the server stops or restarts and the driver's event stream closes
- **THEN** the driver SHALL emit one `error` line with code `server_lost` and a `resume` object naming the driven Session and the command that continues it, without the password
- **AND** it SHALL settle each waiting input as not answered, end with an `end` line carrying the same `error_code` and `resume`, and exit 2
- **AND** it SHALL NOT reconnect, resubmit input or reply to requests on the restarted server; the admitted input stays in the Session once

#### Scenario: EOF with a pending approval
- **WHEN** the driver's input closes while one of its requests awaits a reply
- **THEN** that request follows the defined disconnected/non-interactive policy and the process does not wait forever or fabricate approval

#### Scenario: Last-message publication collides or fails
- **WHEN** a driver publishes its final assistant text to `--output-last-message`
- **THEN** it SHALL stage bytes in an exclusively created sibling file and atomically replace the destination without following a pre-existing temporary or destination link
- **AND** concurrent writers SHALL publish complete answers and clean up their own temporary files, including after publication failure
- **AND** a publication failure SHALL preserve per-input results, report the error in `end`, and fail an otherwise successful aggregate run without replacing an existing interruption or protocol exit code

#### Scenario: Owned requests and buffered input stay bounded
- **WHEN** an owned request arrives while 64 owned requests already wait for this driver
- **THEN** the driver SHALL decline it at once (`reject` for a permission, dismissal for a question) and write an `error` line with code `capacity` naming the request, without tracking it or ending the run
- **AND** at most 16 parsed stdin lines SHALL wait for the control loop, so a writer that runs further ahead is held back by its own pipe

#### Scenario: Step lines name the inputs they answer
- **WHEN** the driver writes an `assistant_text`, `reasoning`, `tool_use`, `tool_result`, `permission_denied`, `retry`, `text_delta`, `reasoning_delta` or `usage_limit` line for a step whose recorded input cohort belongs to it, or the `error` line of that step's failure
- **THEN** the line SHALL carry that step's `input_message_ids`; `system`, `input`, `result` and `end` lines and protocol errors SHALL NOT, and request lines keep their server-derived provenance

#### Scenario: Error text is secret-safe
- **WHEN** a JSONL line carries diagnostic text in `error` or `message`, including provider error bodies, server responses and echoed stdin
- **THEN** the driver SHALL replace every secret found by the built-in detectors, the configured `tool_output.redact` patterns (minus its allow list) and the exact values of known secrets (secret-named environment variables, secret configuration entries and the attach password) with its redaction marker, also when `tool_output.redact.enabled` is `false`
- **AND** answer text in `result` SHALL NOT be altered

#### Scenario: Completed inputs do not consume pending capacity
- **WHEN** a driver emits terminal results across a run longer than its pending-input limit
- **THEN** it SHALL release completed text, structured results and correlation records, retaining only bounded aggregate accounting
- **AND** the limit SHALL apply before each HTTP admission to pending inputs, completion SHALL preserve the exact identities of other pending inputs, and `end.inputs` SHALL still count retired attempts
- **AND** retiring a failed input SHALL NOT erase its aggregate failure; premature termination SHALL emit a terminal result for every remaining pending input

#### Scenario: EOF during an owned handback question
- **WHEN** EOF closes a driver while a handback question belongs to its retained original input
- **THEN** it SHALL decline that exact request and drain its resulting answer under the original input identity without hanging
- **AND** an input stopped without a recorded final answer SHALL fail unless a valid structured outcome already establishes its terminal result; a valid structured object containing null fields SHALL remain successful
