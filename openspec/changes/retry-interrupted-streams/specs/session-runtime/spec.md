## MODIFIED Requirements

### Requirement: Eager tool execution
(P1) The system SHALL start each complete tool call while the provider stream continues, when the tool is marked `concurrency_safe` and its permission evaluates to `allow`. Before starting it, the system SHALL durably record the answer streamed so far, which ends automatic replay of that answer (`provider-catalog` → Retry policy). It SHALL await all started calls after the stream closes, and run non-safe calls sequentially in emitted order.

#### Scenario: Parallel reads during streaming
- **WHEN** the model emits three `read` calls followed by more text
- **THEN** all three reads start before the stream finishes

#### Scenario: Started read ends replay
- **WHEN** a `read` call has started during streaming and the stream then fails with a `Transport` error
- **THEN** the request is not replayed and the Turn is settled as interrupted with the recorded `read` outcome
