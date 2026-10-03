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
(P0) The system SHALL run at most one Drain per Session across all processes on a machine, using an in-process coordinator plus an advisory lock in `<state>/locks/<session_id>.lock`, while different Sessions run concurrently. A second process attempting to drain a locked Session SHALL admit its input and wake the owner through the server instead of draining.

#### Scenario: TUI and exec on the same Session
- **WHEN** `cyber exec -s ses_1 "..."` runs while the TUI drains `ses_1`
- **THEN** the exec prompt is admitted and executed by the TUI's Drain, and exec streams the result from the event stream

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
(P0) The system SHALL record agent, model/variant, and mode switches as durable events (`session.agent.switched.1`, `session.model.switched.1`, `session.mode.switched.1`) that take effect at the next Turn and append history markers that are not sent to the model. Switching to the current selection SHALL be a no-op.

#### Scenario: Switch model mid-session
- **WHEN** the user switches from `openai/gpt-6` to `anthropic/claude-sonnet` during a Turn
- **THEN** the current Turn completes on `openai/gpt-6` and the next Turn uses `anthropic/claude-sonnet`

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
(P0) The system SHALL fork a Session from an optional message (copying history before it with fresh IDs into a new Session titled `<title> (fork #N)`), rename it, archive it (hidden from default lists but resumable), and delete it (cascading to child Sessions, inbox rows, snapshots metadata, and published `session.deleted.1` events).

#### Scenario: Fork from a message
- **WHEN** the user forks at message 12 of 20
- **THEN** a new Session contains copies of messages 1–11 and the original is unchanged

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
