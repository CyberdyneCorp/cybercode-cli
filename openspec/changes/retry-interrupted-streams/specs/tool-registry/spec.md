## MODIFIED Requirements

### Requirement: Eager execution during streaming
(P1) The system SHALL durably record each complete tool call no later than when the provider stream completes, and always before the call starts. It MAY start `concurrency_safe` calls before the provider stream ends. Starting one SHALL first record the answer streamed so far, and SHALL end automatic replay of that answer (`provider-catalog` → Retry policy). It SHALL await all started calls before the next Turn. A complete call of an attempt that is discarded before any call started SHALL NOT be recorded.

#### Scenario: Read starts before stream ends
- **WHEN** the model streams a complete `read` call followed by more text
- **THEN** the read may begin immediately and its result is present before the next Turn

#### Scenario: Complete call of a discarded attempt
- **WHEN** the model streams a complete `write` call, and the body fails with a `Transport` error before protocol completion
- **THEN** no `session.tool.called.1` is recorded for that attempt
- **AND** the replayed attempt's `write` call is recorded once, before it is dispatched
