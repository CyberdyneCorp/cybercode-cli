## MODIFIED Requirements

### Requirement: Exact-retry idempotency
(P0) The system SHALL return the original admission receipt when a prompt is resubmitted with the same `message_id`, Session, prompt content, and delivery, and SHALL fail with `PromptConflictError` (HTTP 409) when that `message_id` was used with different content or delivery, or identifies a message outside this admission record. An identical retry SHALL return the original receipt even after promotion.

#### Scenario: Client retries after timeout
- **WHEN** a mobile client resends the same `message_id` and identical prompt after a network timeout
- **THEN** it receives the original receipt and the prompt appears in history once

### Requirement: Drain loop
(P0) The system SHALL run a Drain that executes Turns while the last Turn produced tool calls without a provider error, or while promotable `steer` input exists, then promotes the next `queue` input if any and repeats, and ends when nothing is eligible. Before its first Turn, a Drain SHALL reconcile unfinished calls using the tool recovery contract: calls never dispatched may be interrupted safely; dispatched mutations without a recorded outcome SHALL become `outcome_unknown` and SHALL NOT be automatically repeated.

#### Scenario: Crash recovery
- **WHEN** the server restarts while a `bash` call was running
- **THEN** the next Drain records `outcome_unknown` when execution was dispatched without a durable outcome and requires reconciliation before repeating the mutation

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
