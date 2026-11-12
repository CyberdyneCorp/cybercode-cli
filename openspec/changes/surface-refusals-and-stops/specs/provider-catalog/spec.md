## ADDED Requirements

### Requirement: Refusals and finish reasons
(P1) A model's refusal SHALL reach the user as the answer's text: the OpenAI Responses adapter SHALL treat `response.refusal.delta` and the Chat Completions adapter SHALL treat `delta.refusal` as text deltas. Each finished assistant message SHALL carry `finish`, the step's finish reason (`stop`, `tool_calls`, `length`, `content_filter` or another provider value), in the messages API; it SHALL be null while the step runs and when it failed.

#### Scenario: Streamed refusal
- **WHEN** a Responses stream sends `response.refusal.delta` with "I can't help with that." and ends incomplete with reason `content_filter`
- **THEN** the assistant message's text is "I can't help with that." and its `finish` is `content_filter`
