# compaction Specification

## Purpose
Compaction keeps long Sessions within a model's context window by replacing older history with a structured summary while keeping a recent tail verbatim, then starting a new Context Epoch. It merges OpenCode v1/v2 overflow handling and tail retention with Claude Code's manual `/compact [instructions]` and PreCompact/PostCompact hooks. Full history stays in storage; only the model's view changes.

## Requirements

### Requirement: Compaction configuration
(P0) The system SHALL read `compaction` config with keys `auto` (default true), `buffer` (tokens, default 20000), `keep.tokens` (default 8000), `keep.turns` (optional cap on retained user turns), `prune` (default false), and `model` (falls back to the `compaction` model role). `CYBER_DISABLE_AUTOCOMPACT=1` SHALL force `auto` to false.

#### Scenario: Disabled by environment
- **WHEN** `CYBER_DISABLE_AUTOCOMPACT=1` is set
- **THEN** no automatic compaction runs regardless of config

### Requirement: Automatic compaction trigger
(P0) The system SHALL compact at a Safe Boundary before a Turn when `auto` is true and the estimated request size exceeds the model context limit minus the greater of the model's output allowance and `compaction.buffer`.

#### Scenario: Approaching the window
- **WHEN** the estimated next request is 185,000 tokens on a 200,000-token model with a 20,000-token buffer
- **THEN** compaction runs before the Turn is sent

### Requirement: Overflow-triggered compaction
(P0) The system SHALL attempt exactly one compaction and rebuild the same Turn when the provider reports `ContextOverflow` before any assistant output, unless `auto` is false, in which case the error SHALL be stored on the assistant message and the Drain SHALL stop.

#### Scenario: Provider rejects the request
- **WHEN** a request fails with `ContextOverflow` and no output was produced
- **THEN** the Session compacts once and retries the Turn

### Requirement: Never compact mid-tool-call
(P0) The system SHALL only compact at Safe Boundaries, after all tool calls of the previous Turn are settled, and SHALL defer a compaction requested during a Turn until the next Safe Boundary.

#### Scenario: Manual compaction during a Turn
- **WHEN** the user runs `/compact` while tools are executing
- **THEN** compaction starts after the current Turn's tool calls settle

### Requirement: Recent tail retention
(P0) The system SHALL keep the most recent history verbatim up to `keep.tokens` tokens (default 8000), limited to the last `keep.turns` user turns when set, never splitting a tool call from its result, and SHALL record the tail start on the compaction record.

#### Scenario: Tail boundary respects tool pairs
- **WHEN** the token budget would end between a tool call and its result
- **THEN** the tail starts before the tool call

### Requirement: Summary generation
(P0) The system SHALL generate the summary with the compaction model, with no tools, from the history before the tail serialized as role-labeled lines (tool output truncated to 2000 characters each), requesting Markdown sections `Objective`, `Important details`, `Work state` (completed, active, blocked), `Next move`, and `Relevant files`, and including any previous summary to merge.

#### Scenario: Second compaction merges the first
- **WHEN** a Session compacts for the second time
- **THEN** the previous summary is included as prior context and the new summary supersedes it

### Requirement: Manual compaction with instructions
(P0) The system SHALL support `/compact [instructions]` and `POST /api/v1/sessions/{id}/compact` with optional `instructions`, appending the instructions to the summary request (for example "focus on the database migration").

#### Scenario: Focused summary
- **WHEN** the user runs `/compact keep details about the auth refactor`
- **THEN** the summary request includes that instruction

### Requirement: Model view after compaction
(P0) The system SHALL hide history before the latest completed compaction from model requests, presenting the summary as a user-role message, then the retained tail and all later messages, while keeping the full history in storage and in client views.

#### Scenario: Scrolling back in the TUI
- **WHEN** the user scrolls above the compaction marker
- **THEN** the original messages are still displayed but are not sent to the model

### Requirement: New Context Epoch
(P0) The system SHALL start a new Context Epoch on the first Turn after a completed compaction, rendering a fresh baseline from all current Context Sources, as defined by the system-context capability.

#### Scenario: Instructions refreshed after compaction
- **WHEN** `AGENTS.md` changed earlier and the update was sent mid-conversation
- **THEN** after compaction the baseline contains the new instructions and the earlier update message is no longer projected

### Requirement: Auto-continue after automatic compaction
(P0) The system SHALL, after a successful automatic compaction triggered during an active Drain, continue the Drain without user input by adding a synthetic instruction to continue the current work, and SHALL NOT auto-continue after a manual compaction.

#### Scenario: Seamless continuation
- **WHEN** automatic compaction runs in the middle of a long task
- **THEN** the agent resumes the task in the next Turn without a new user prompt

### Requirement: Media stripping on overflow
(P0) The system SHALL, when compaction was triggered by overflow, replace images and PDFs in the retained tail and the replayed Turn with `[Attached <mime>: <name>]` placeholders.

#### Scenario: Large screenshot
- **WHEN** an overflow was caused by several screenshots in recent messages
- **THEN** the retried Turn carries text placeholders instead of the images

### Requirement: Compaction failure
(P0) The system SHALL fail a compaction whose summary request itself overflows or errors by recording the error on the compaction record, publishing `session.compaction.failed.1`, and stopping the Drain with the message `Session too large to compact; start a new session or fork from an earlier message`.

#### Scenario: Summary request too large
- **WHEN** the history before the tail cannot fit the compaction model's window
- **THEN** the Drain stops with the compaction failure message

### Requirement: Compaction events
(P0) The system SHALL record a durable `session.compaction.started.1` and `session.compaction.completed.1` (with summary message ID, tail start, tokens before and after, and trigger `auto`, `overflow`, or `manual`), and SHALL show a compaction marker in clients.

#### Scenario: Token savings reported
- **WHEN** a compaction completes
- **THEN** the completed event records tokens before and after so clients can show the savings

### Requirement: Compaction hooks
(P1) The system SHALL run `PreCompact` hooks before the summary request (allowing them to add instructions or block an automatic compaction) and `PostCompact` hooks after completion, as defined by the hooks capability.

#### Scenario: Hook adds context
- **WHEN** a `PreCompact` hook returns `additional_instructions: "keep the TODO list"`
- **THEN** the summary request includes that text

### Requirement: Tool output pruning
(P1) When `compaction.prune` is true, the system SHALL, after each Drain, replace outputs of completed tool calls older than the two most recent user turns with `[output pruned]` (keeping the newest 40,000 estimated tokens of tool output and skipping `skill` and `memory` outputs), only when more than 20,000 tokens would be freed.

#### Scenario: Old test logs pruned
- **WHEN** pruning is enabled and old `bash` outputs total 60,000 tokens
- **THEN** the oldest outputs are replaced by `[output pruned]` until 40,000 tokens remain

### Requirement: Compaction in subagents and workflows
(P2) The system SHALL apply the same compaction rules to subagent and workflow agent Sessions, using the parent's compaction model role unless the agent defines its own, and SHALL report compactions in the Workflow Run monitor.

#### Scenario: Long-running workflow agent
- **WHEN** a workflow agent's Session exceeds its compaction threshold
- **THEN** it compacts and continues, and the run monitor shows one compaction for that agent

### Requirement: Durable task state
(P0) The runtime SHALL persist a versioned Session task-state record containing the objective, user constraints with source message IDs, accepted decisions, open questions, active work and verification evidence (command, exit status, timestamp and workspace content hash). Permission decisions and trust grants SHALL remain authoritative in their own stores and SHALL never be inferred from a summary. Compaction SHALL carry the task-state record separately from prose, preserve provenance and unresolved tool outcomes, and SHALL NOT remove a constraint without a later user instruction identifying its supersession. Model-proposed state changes SHALL retain the supporting source; they SHALL NOT create approval grants. Evidence SHALL be marked stale when relevant files change.

#### Scenario: Repeated compaction preserves a boundary
- **WHEN** a user says do not deploy and the Session compacts three times
- **THEN** the original constraint and source message ID remain in task state and no summary grants deployment permission

### Requirement: Retrieval of earlier context
(P0) The read-only `history_search` tool SHALL accept `{ query, before_seq?, limit? }` (default 10, maximum 50) and return matching excerpts with message IDs and sequence numbers from the current Session, subject to normal tool output limits. It SHALL also accept `{ message_id }` to retrieve a cited source. Compaction SHALL retain original messages for this purpose until explicit deletion or retention expiry. Search results SHALL preserve original roles and provenance and SHALL NOT be promoted to instructions. Child Sessions SHALL NOT gain access to unrelated Sessions through search.

#### Scenario: Recover an old decision
- **WHEN** the agent searches for a decision omitted from the current summary
- **THEN** it receives the original excerpt and source ID from its own Session
