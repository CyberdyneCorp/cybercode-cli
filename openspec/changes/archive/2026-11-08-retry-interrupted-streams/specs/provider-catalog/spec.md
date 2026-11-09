## MODIFIED Requirements

### Requirement: Retry policy
(P0) The system SHALL retry `RateLimit`, `ProviderInternal`, and `Transport` failures up to 4 times, waiting `retry-after-ms` or `retry-after` when present (capped at 60 seconds) and otherwise using jittered exponential backoff starting at 1 second (factor 2, cap 30 seconds), and SHALL never retry `ContextOverflow`, `Authentication`, `ContentPolicy`, or `InvalidRequest`. Each retry SHALL publish a live `session.retry` event with the step's `message_id`, the attempt number, the delay, the error and `discarded`. Retries before and after assistant output SHALL share these 4 retries.

A retryable failure after partial output SHALL discard that attempt and send the same request again, with `discarded: true`. Partial output covers text, reasoning, streamed tool-call arguments and complete tool calls of the answer. This replay SHALL apply only before protocol completion and before any tool call of the answer has been dispatched. The idle timeout and in-stream overload errors are such failures. The runtime SHALL NOT dispatch a tool call of an answer while its stream can still be replayed. Calls SHALL dispatch after the stream completes, and starting a call early (`tool-registry` → Eager execution during streaming) SHALL end replay for that answer.

No event of a discarded attempt SHALL be recorded as part of the answer. Reasoning items, text and tool calls SHALL be recorded only when the stream completes or fails for good. A restart, the next request and every client SHALL see only the attempt that ended the step, with reasoning items numbered from 0.

Usage that a discarded attempt reported SHALL be recorded as a failed `usage.recorded.1`, with purpose `turn` and the step's message ID as `call_id`, unless a budget reservation already billed that attempt. It SHALL count toward Session totals and budgets, but not toward the step's own usage or the context meter.

Each attempt SHALL pass the same budget checks and reservation admission as a new request, and no attempt SHALL hold a reservation while another attempt of the same step does. The Turn's `x-codex-turn-state` value SHALL be kept across the replay.

Clients SHALL drop the live text, reasoning and tool input of `message_id` when they receive `discarded: true`.

When retries are exhausted, or a failure after partial output is not retryable, the Turn SHALL be settled as interrupted with the final attempt's partial output. Undispatched calls SHALL be settled as interrupted, and tool outcomes SHALL be reconciled before a new Turn. The system SHALL NOT replay a request after any of its tool calls was dispatched or after protocol completion.

#### Scenario: Retry-after honored
- **WHEN** a 429 response carries `retry-after: 7`
- **THEN** the next attempt starts after 7 seconds

#### Scenario: Overflow not retried
- **WHEN** the error reason is `ContextOverflow`
- **THEN** no retry is attempted and the compaction capability decides what happens next

#### Scenario: HTTP body fails after protocol completion
- **WHEN** a provider stream has emitted its protocol completion, including final usage
- **AND** the HTTP body stalls or closes incorrectly afterward
- **THEN** the completed response remains successful without waiting for the HTTP tail or replaying the request

#### Scenario: Body interruption before protocol completion
- **WHEN** an HTTP response body fails before protocol completion
- **THEN** the transport diagnostic identifies the response-stream stage and received body-byte count without adding response content or credentials
- **AND** failures before assistant output are retried within the bounded retry policy
- **AND** after partial text, reasoning, tool arguments or completed but undispatched tool calls, the attempt is discarded and the same request is replayed within the same budget
- **AND** once the budget is spent, the final attempt's partial output is kept, its undispatched calls are settled as interrupted, and completed tool outcomes stay settled across restart and explicit continuation

#### Scenario: Partial answer replaced by the replay
- **WHEN** a Turn streams "Here is the first half" and a completed `write` call, and then the connection resets
- **AND** the replayed request answers "Done." and the same `write` call
- **THEN** clients receive `session.retry` with `discarded: true` and drop the first half
- **AND** the recorded step holds only "Done." and one `write` call, before and after a restart
- **AND** `write` runs once

#### Scenario: Usage of a discarded attempt
- **WHEN** an attempt without a budget reservation reports 1,200 input tokens and then fails mid-answer with `ProviderInternal` `overloaded_error`
- **THEN** a failed `usage.recorded.1` with purpose `turn` records the 1,200 input tokens and their cost
- **AND** the step's `session.step.ended.1` usage and the context meter hold only the replayed attempt's usage
