# session-runtime Specification

## Purpose
The session runtime records Sessions and their durable input inbox, then runs admitted work through Drains of Turns on the Session's Location. It adopts the OpenCode v2 model (durable admission before execution, steer and queue delivery, Safe Boundaries, replayable events) and adds `hold` delivery, side chats from Codex, and the hook points that goals, loops, messaging, and channels plug into (from Claude Code). Every model-visible fact is persisted before it is acted on, so execution can be resumed, joined, or interrupted without losing input.

## Requirements

### Requirement: Idempotent Session creation
(P0) The system SHALL create a Session from a required Location and optional caller-supplied `id` (prefix `ses_`), `agent`, `model`, `mode`, `parent_id`, and `title`. Creating with an `id` that already exists SHALL return the existing Session unchanged without publishing a new `session.created.1` event. A new Session SHALL start with zero cost and token totals and the title `New session - <ISO timestamp>`.

#### Scenario: Reuse an existing ID
- **WHEN** a client creates a Session with `id: "ses_abc"` and that Session already exists
- **THEN** the existing Session is returned and no creation event is published

### Requirement: Durable prompt admission
(P0) The system SHALL admit input by appending a durable `session.prompt.admitted.1` event that inserts one row into the Session inbox with `{ message_id, prompt, delivery, source, admitted_seq }`, where `source` is `user`, `session` (cross-session message), `channel`, `loop`, `goal`, `workflow`, or `hook`. Admission SHALL NOT append the user message to model-visible history.

#### Scenario: Admission before execution
- **WHEN** a user submits a prompt while the server is about to crash
- **THEN** after restart the prompt is still in the inbox and is promoted by the next Drain

### Requirement: Exact-retry idempotency
(P0) The system SHALL return the original admission receipt when a prompt is resubmitted with the same `message_id`, Session, prompt content, and delivery, and SHALL fail with `PromptConflictError` (HTTP 409) when that `message_id` was used with different content or delivery, or identifies a message outside this admission record. An identical retry SHALL return the original receipt even after promotion.

#### Scenario: Client retries after timeout
- **WHEN** a mobile client resends the same `message_id` and identical prompt after a network timeout
- **THEN** it receives the original receipt and the prompt appears in history once

### Requirement: Delivery modes
(P0) The system SHALL support `delivery` values `steer` (default; promoted at the next Safe Boundary even while the Drain continues), `queue` (FIFO; promoted one at a time only when the Session would otherwise go idle), and `hold` (recorded but not promotable until approved). Approving a held input SHALL convert it to `steer` or `queue`, and rejecting it SHALL mark it `refused` without promotion.

#### Scenario: Steer during a long Turn chain
- **WHEN** the user sends a `steer` prompt while the agent is running tools
- **THEN** the prompt is promoted after the current Turn settles, before the next provider request

#### Scenario: Held message awaiting approval
- **WHEN** a cross-session message arrives with `delivery: hold`
- **THEN** it is visible in the inbox as `held` and is not sent to the model until the user approves it

### Requirement: Inbox management
(P0) The system SHALL let the owner list unpromoted inbox rows and edit or remove a `queue` or `hold` row before promotion (`PATCH|DELETE /api/v1/sessions/{id}/inbox/{message_id}`), publishing `session.inbox.updated.1`. Promoted rows SHALL be immutable.

#### Scenario: Take back a queued prompt
- **WHEN** the user deletes a queued prompt before it is promoted
- **THEN** it is removed from the inbox and never reaches the model

### Requirement: Prompt promotion at Safe Boundaries
(P0) The system SHALL promote eligible inbox rows only at Safe Boundaries (before the first Turn of a Drain and between Turns after all tool calls of the previous Turn are settled) by appending `session.prompt.promoted.1`, which atomically marks the row with `promoted_seq` and appends the user message to history. At each Safe Boundary the system SHALL apply, in order: promoted inputs, context updates (`system-context`), goal evaluation results (`goals`), and loop or cron firings (`loops-scheduling`).

#### Scenario: Several steer inputs at one boundary
- **WHEN** three `steer` prompts were admitted during one Turn
- **THEN** all three are promoted at the next Safe Boundary in admission order

### Requirement: Drain loop
(P0) The system SHALL run a Drain that executes Turns while the last Turn produced tool calls without a provider error, or while promotable `steer` input exists, then promotes the next `queue` input if any and repeats, and ends when nothing is eligible. Before its first Turn, a Drain SHALL reconcile unfinished calls using the tool recovery contract: calls never dispatched may be interrupted safely; dispatched mutations without a recorded outcome SHALL become `outcome_unknown` and SHALL NOT be automatically repeated.

#### Scenario: Crash recovery
- **WHEN** the server restarts while a `bash` call was running
- **THEN** the next Drain records `outcome_unknown` when execution was dispatched without a durable outcome and requires reconciliation before repeating the mutation

### Requirement: Wake versus resume
(P0) The system SHALL provide `wake(session)`, which starts a non-forced Drain when idle or records one coalesced follow-up when a Drain is active, and `resume(session)`, which joins an active Drain or starts a forced Drain that performs one Turn even with no new input. Every successful admission SHALL wake the Session unless the caller passed `resume: false`.

#### Scenario: Admission without execution
- **WHEN** a client admits a prompt with `resume: false`
- **THEN** the prompt waits in the inbox and no Turn starts

### Requirement: Per-Session serialization
(P0) The system SHALL run at most one Drain per Session. Drains SHALL run only in the registered server that holds the writer ownership lock (`storage-events`), coordinated in-process, while different Sessions run concurrently. Any other process (TUI, `cyber exec`, SDK) that wants a Session drained SHALL admit its input through the server API and wake the owner; it SHALL NOT run a Drain of its own against the shared database. An `--embedded` process owns a private database and drains only its own Sessions.

#### Scenario: TUI and exec on the same Session
- **WHEN** `cyber exec -s ses_1 "..."` runs while the TUI is attached to `ses_1`
- **THEN** the exec prompt is admitted and executed by the server's Drain, and exec streams the result from the event stream

### Requirement: Interrupt
(P0) The system SHALL interrupt a Session by stopping its Drain, clearing coalesced wakes, settling undispatched calls as interrupted and dispatched mutations with no known outcome as `outcome_unknown`, failing an active assistant step with `Provider turn interrupted`, and keeping all inbox rows. Interrupting an idle Session SHALL be a no-op. Background tasks owned by the Session SHALL keep running unless the caller passes `stop_background: true`.

#### Scenario: Escape during tool execution
- **WHEN** the user presses Esc while `bash` runs
- **THEN** the command's process group is terminated, the call records any known outcome or `outcome_unknown` if its side effects cannot be established, and the Session becomes idle

### Requirement: Turn assembly
(P0) The system SHALL send exactly one provider stream request per Turn, built from the resolved model and variant, the agent system prompt followed by the Context Epoch baseline, the projected history after the latest compaction, mid-conversation system messages, and the tool definitions materialized for the agent and mode. History SHALL project undispatched interrupted calls as `[Tool execution was interrupted]` and unresolved dispatched mutations as `[Tool outcome unknown; reconcile before retrying]`, preserving their call IDs.

#### Scenario: Interrupted call in history
- **WHEN** history contains a tool call interrupted in an earlier Drain
- **THEN** the next request presents it to the model as an error result

### Requirement: Durable projection of provider output
(P0) The system SHALL persist each Turn as one assistant message through durable events `session.step.started.1`, `session.text.ended.1`, `session.reasoning.ended.1`, `session.tool.called.1`, `session.tool.settled.1`, and `session.step.ended.1` (finish reason, usage, cost) or `session.step.failed.1`. Text, reasoning, and tool-input deltas SHALL be live-only events.

#### Scenario: Reconnect mid-stream
- **WHEN** a client reconnects while text is streaming
- **THEN** it receives durable events through replay and live deltas from the reconnect point onward

### Requirement: Eager tool execution
(P1) The system SHALL durably record each complete tool call and start it while the provider stream continues, when the tool is marked `concurrency_safe` and its permission evaluates to `allow`, await all started calls after the stream closes, and run non-safe calls sequentially in emitted order.

#### Scenario: Parallel reads during streaming
- **WHEN** the model emits three `read` calls followed by more text
- **THEN** all three reads start before the stream finishes

### Requirement: Unknown and stale tool calls
(P0) The system SHALL settle a call to a tool not advertised in the Turn with `Unknown tool: <name>` and a call whose registration was removed before settlement with `Stale tool call: <name>`, without invoking any handler. A name that matches an advertised tool only after lowercasing SHALL be repaired to that tool.

#### Scenario: Case-mismatched tool name
- **WHEN** the model calls `Read`
- **THEN** the call is executed as `read`

### Requirement: Step limit
(P0) The system SHALL bound each Drain by the agent's `steps` (default unlimited for primary agents, 50 for subagents). When the limit is reached it SHALL send one final Turn with no tools and an instruction to summarize completed and remaining work, and fail any tool call emitted in that Turn with `Tools are disabled after the maximum agent steps`. Promoting new user input SHALL reset the step count.

#### Scenario: Subagent reaches 50 steps
- **WHEN** a subagent completes its 50th Turn with tool calls pending
- **THEN** its next Turn has no tools and returns a summary to the parent

### Requirement: Agent, model, and mode switching
(P0) The system SHALL record agent, model/variant, mode and output style switches as durable events (`session.agent.switched.1`, `session.model.switched.1`, `session.mode.switched.1`, `session.output_style.switched.1`). They SHALL take effect at the next Turn and append history markers that are not sent to the model. Switching to the current selection SHALL be a no-op. An output style switch SHALL record the Session's own selection (`null` returns to the configured style) and SHALL be refused for Sessions with a parent.

#### Scenario: Switch model mid-session
- **WHEN** the user switches from `openai/gpt-6` to `anthropic/claude-sonnet` during a Turn
- **THEN** the current Turn completes on `openai/gpt-6` and the next Turn uses `anthropic/claude-sonnet`

#### Scenario: Output style survives a restart
- **WHEN** the user switches a Session to output style `concise` and the server restarts
- **THEN** the Session still reports `output_style: "concise"` and its next Turn uses that style

### Requirement: Reasoning lowering across models
(P0) The system SHALL send historical reasoning and provider metadata (signatures, encrypted reasoning) natively only when the historical message's provider and model exactly match the Turn's model, and otherwise SHALL lower non-empty reasoning to plain assistant text and drop empty reasoning.

#### Scenario: Reasoning after a provider switch
- **WHEN** history holds Anthropic thinking blocks and the Turn uses OpenAI
- **THEN** the thinking text is sent as ordinary assistant text without signatures

### Requirement: Usage and cost
(P0) The system SHALL accumulate per-step token usage (input, output, reasoning, cache read, cache write) and cost on the assistant message and on the Session totals, and SHALL publish live `session.usage` events with context-window utilization after each step.

#### Scenario: Context utilization shown
- **WHEN** a step uses 120,000 tokens of a 200,000-token window
- **THEN** the usage event reports 60% utilization

### Requirement: Title generation
(P0) The system SHALL generate a title for a root Session after its first promoted user message when the title is still the default, using the `title` model role with no tools and at most 80 output tokens, trimming to 100 characters and stripping reasoning tags. Title failure SHALL keep the default title without failing the Session.

#### Scenario: Title from first prompt
- **WHEN** the first prompt asks to fix a flaky login test
- **THEN** the Session receives a short generated title such as `Fix flaky login test`

### Requirement: Fork, rename, archive, delete
(P0) The system SHALL fork a Session from an optional message (copying history before it with fresh IDs into a new Session titled `<title> (fork #N)`), rename it, archive it (hidden from default lists but resumable), and delete it (cascading to child Sessions, inbox rows, snapshots metadata, and published `session.deleted.1` events). A user-set title SHALL be trimmed, with runs of whitespace (newlines included) joined into one space, control characters removed and at most 100 characters kept; a title that is empty after cleaning SHALL fail with `InvalidRequestError` and change nothing. Once a user sets a title, a generated title SHALL never replace it, including the result of a title request already in flight.

#### Scenario: Fork from a message
- **WHEN** the user forks at message 12 of 20
- **THEN** a new Session contains copies of messages 1–11 and the original is unchanged

#### Scenario: Rename is cleaned
- **WHEN** a client renames a Session to `"  Auth\n refactor\u001b]0;x\u0007 "`
- **THEN** its title is `Auth refactor]0;x`

#### Scenario: Empty rename refused
- **WHEN** a client renames a Session to `" \t "`
- **THEN** the request fails with `InvalidRequestError` and the title is unchanged

#### Scenario: User title survives title generation
- **WHEN** the user renames a Session to `Mine` while the title request for its first prompt is still running
- **THEN** the generated title is discarded and the title stays `Mine`

### Requirement: Session listing and paging
(P0) The system SHALL list Sessions filtered by directory, project, workspace, parent, archived flag, and title search, ordered by `updated_at` descending by default, with opaque cursors and a limit of 1–200 (default 50), and SHALL page a Session's messages by durable sequence with the same cursor contract.

#### Scenario: Next page
- **WHEN** a client requests the next page with the returned cursor
- **THEN** it receives the following 50 Sessions without duplicates

### Requirement: Durable event replay
(P0) The system SHALL expose each Session's durable events as a finite page (`GET /api/v1/sessions/{id}/history?after=<seq>&limit=<1..500>`) and as an SSE stream (`GET /api/v1/sessions/{id}/events?after=<seq>`) that replays events after the exclusive sequence and then tails new durable events without gaps.

#### Scenario: Lossless reconnect
- **WHEN** a client reconnects with `after=1042`
- **THEN** it receives every durable event with sequence 1043 and higher, in order

### Requirement: Structured output
(P1) The system SHALL accept a per-prompt `format: { type: "json_schema", schema }`, add a `structured_output` tool built from the schema, require it as the final call, validate the input against the schema (returning validation errors to the model up to 3 times), store the validated value on the assistant message, and end the Drain.

#### Scenario: Schema enforced
- **WHEN** a prompt requests `{ type: "object", required: ["summary"] }`
- **THEN** the Turn ends with a validated object containing `summary` stored as structured output

### Requirement: User shell commands
(P0) The system SHALL run a prompt beginning with `!` as a user shell command in the Session's Location using the configured shell, under the sandbox but without permission prompts, record a synthetic user message and a `bash` tool part with the output, and make that output visible to the model in the next Turn.

#### Scenario: Run tests from the prompt
- **WHEN** the user submits `!cargo test`
- **THEN** the command output is recorded in history without starting a model Turn

### Requirement: Side chat
(P1) The system SHALL support an ephemeral side chat (`/btw <question>`) that sends the question with a read-only copy of the current context to the Session's model using read-only tools only, shows the answer to the user, and records nothing in the Session's model-visible history.

#### Scenario: Quick question while working
- **WHEN** the user runs `/btw what does this regex match?` during a Drain
- **THEN** the answer is shown in a side panel and the main Drain's history is unchanged

### Requirement: Attachment normalization
(P0) Prompt attachments (TUI paste or drop, `@path` mentions, `--file`, remote uploads) SHALL obey the `attachments` config:

- `max_bytes`: default 20 MiB per file. Larger files are rejected with `AttachmentTooLargeError`, and a file part is checked before it is read.
- `image.max_dimension`: default 2000 px. Larger images are downscaled, preserving the aspect ratio.
- `image.max_bytes`: default 5 MiB. Images above it are re-encoded as JPEG quality 85.
- `pdf.max_pages`: default 100. Longer PDFs are attached with a page-range prompt.

The media type SHALL be taken from the bytes, never from the client's `media_type` or the file extension. Only PNG, JPEG, GIF and WebP images and PDF documents SHALL be accepted, and anything else SHALL be rejected with `UnsupportedAttachmentError`. Images SHALL be decoded under limits of 100 megapixels and 512 MiB of allocation, and an image over them SHALL be rejected before its pixels are decoded. Normalization SHALL be deterministic.

Normalized attachments SHALL be stored as managed artifacts (`storage-events` → Attachment storage) referenced from the message part. The original SHALL be kept when `attachments.keep_original` is true (default false).

The Session's model is resolved at admission:

- A prompt with an image SHALL be refused with `UnsupportedModalityError` when the model lacks `capabilities.vision`, and one with a PDF when it lacks `capabilities.pdf`. The message SHALL be `<provider>/<model> has no <image|pdf> input; switch models with /model or remove the attachment`, and nothing SHALL be admitted.
- When a later Turn runs on a model without the input, a model switch or a fallback, earlier attachments SHALL reach that model as the placeholder defined by `tool-registry`.
- An attachment whose blob is missing SHALL reach the model as `[<mime> <name> missing from storage]`.

Attachments SHALL never be dropped without a placeholder.

#### Scenario: Large screenshot downscaled
- **WHEN** the user pastes a 4000×3000 PNG of 9 MiB
- **THEN** the stored attachment is at most 2000 px on its longest side and under 5 MiB, and the model receives the downscaled image

#### Scenario: Extension does not decide the type
- **WHEN** a client attaches a ZIP archive renamed to `photo.png`
- **THEN** the prompt is refused with `UnsupportedAttachmentError` and nothing is stored

#### Scenario: Text-only model refuses an image
- **WHEN** the Session's model has `capabilities.vision` false and a prompt carries an image
- **THEN** the prompt is refused with `UnsupportedModalityError` naming the model, and no Turn starts

#### Scenario: Model switched after an image
- **WHEN** a Session admitted an image on a vision model and then switches to a text-only model
- **THEN** the next Turn's request carries `[image/png omitted: model has no image input]` in place of the image

#### Scenario: Long PDF
- **WHEN** the user attaches a 240-page PDF
- **THEN** the model receives the PDF and the note that only pages 1-100 are expected to be read unless the user names others

### Requirement: Assistant message timing
(P1) Each assistant history entry returned by `GET .../messages` and `GET .../messages/:messageID` SHALL carry `agent` (the agent pinned by `session.step.started.1`), `started_ms` (the time of that event) and `ended_ms` (the time of `session.step.ended.1` or `session.step.failed.1`, null while the Turn runs) and `error_kind` (the `session.step.failed.1` kind, such as `interrupted`, else null). Each user entry SHALL carry `promoted_ms`. The values SHALL come from durable event times, so they are identical after replay or a server restart.

#### Scenario: Timing survives restart
- **WHEN** a Turn starts at 10:00:00.000 and ends at 10:00:11.300, and the server then restarts
- **THEN** the assistant entry still reports `started_ms` and `ended_ms` 11,300 ms apart

### Requirement: Session recap
(P1) The system SHALL generate a Session recap on `POST /api/v1/sessions/:sessionID/recap` with the `small` model role, no tools, at most 120 output tokens, and a prompt asking for one or two sentences in the first person past tense about what was done and what comes next. The result SHALL be trimmed to 400 characters with reasoning tags removed, and stored as a durable `session.recap.generated.1` event with `text`, `model` and `through_seq` (the last durable sequence it covers). The call's usage SHALL be recorded as `usage.recorded.1` with purpose `recap`. When the stored recap already covers the latest finished Turn, the route SHALL return it without a model call. `GET /api/v1/sessions/:sessionID/recap` SHALL return the latest recap or `null`. A Session with no finished Turn SHALL answer `409 ConflictError` with reason `nothing_to_recap`. A failed model call SHALL answer the model error (`503 ServiceUnavailableError` with `service: "model"`) and leave the previous recap in place; a reply that is empty after cleaning records its usage and answers the same way. The recap also carries `generated_ms` (the event time). Concurrent requests for one Session are served one at a time, so they make at most one model call. The prompt holds the newest 16,000 characters of the conversation (user and assistant text, each cut to 2,000 characters, and the names of the tools each assistant message called).

#### Scenario: Cached recap
- **WHEN** a recap exists through the latest finished Turn and a client posts to `/recap` again
- **THEN** the stored recap is returned and no model call is made

#### Scenario: Recap usage is attributed
- **WHEN** a recap is generated
- **THEN** a `usage.recorded.1` event with purpose `recap` is appended and counted in the Session's totals

### Requirement: Session branches
(P1) A Session's messages SHALL form a tree, and the Session SHALL have one active path from its active tip to the root. Only messages on the active path SHALL be sent to the model.

- **Rewind keeps the branch:** a committed conversation rewind (target `conversation` or `both`) to a user message SHALL move the active tip to that message's parent and keep the message and its descendants as an inactive branch. The next admitted prompt SHALL become a sibling of the rewound message.
- **Checkout:** checking out a message SHALL make active the path to the most recently created tip under it, or to the message itself with `exact`. Checkout SHALL require the Session to be idle (no Drain, no promotable input, no compaction in progress and no running background Job or delegation, whose result would be handed back into the newly active path), checked again when the checkout is recorded, and fail with `SessionBusyError` otherwise; a refused checkout SHALL put back any files it restored. When requested, it SHALL restore the working tree to the snapshot after that tip's last step, through the three-phase revert. Checkout SHALL record `session.branch.checked_out.1`.
- **Labels:** labels matching `[A-Za-z0-9._-]{1,48}` SHALL be unique per Session. They SHALL be set or cleared with `session.branch.labeled.1`, and SHALL be usable wherever a message ID is accepted for checkout.
- **Compaction:** a Context Epoch summary SHALL be reused only when the messages it replaced are an ancestor prefix of the active path. Otherwise the context SHALL be assembled from the path's stored messages. While a compaction is in progress, a checkout or the commit of a conversation rewind SHALL fail with `SessionBusyError`, and a summary whose retained tail is not on the active path when it completes SHALL be discarded. A compaction that ends without recording an outcome (an interrupt, a disconnected client) SHALL NOT leave the Session refusing them.
- **Existing Sessions:** replaying a Session whose events contain earlier conversation rewinds SHALL produce the corresponding branches, with the same active path as before. When such a log admits a rewound message ID again, both attempts SHALL be kept: the earlier one, with its descendants, under the derived ID `<id>~<n>`.

#### Scenario: Rewind keeps the old attempt
- **WHEN** a Session has user messages 1, 2, 3, the user rewinds the conversation to message 2, and then sends message 2b
- **THEN** the active path is 1 → 2b, and messages 2 and 3 remain as an inactive branch visible in the tree

#### Scenario: Return to an earlier attempt
- **WHEN** the user checks out message 3 of that inactive branch
- **THEN** the next Turn's context contains messages 1, 2 and 3 and not 2b, and `session.branch.checked_out.1` is recorded

#### Scenario: Checkout while running
- **WHEN** a client checks out a branch while the Session's Drain is running
- **THEN** the request fails with `SessionBusyError` and the active path is unchanged

#### Scenario: Checkout while a background Job runs
- **WHEN** a Turn on branch A started a background Job and a client checks out branch B while the Job runs
- **THEN** the request fails with `SessionBusyError`, and the Job's output never reaches a model request on B

#### Scenario: Checkout during a compaction
- **WHEN** a client checks out sibling branch B while a manual compaction of branch A is writing its summary
- **THEN** the request fails with `SessionBusyError`, and no model request on B contains A's summary, also after a restart

#### Scenario: Summary of a sibling is not reused
- **WHEN** branch A was compacted and the user checks out sibling branch B, which shares only message 1 with A
- **THEN** the context for B is assembled from B's stored messages and contains no part of A's summary
