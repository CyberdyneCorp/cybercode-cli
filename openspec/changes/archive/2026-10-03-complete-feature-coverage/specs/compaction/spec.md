## ADDED Requirements

### Requirement: Native provider compaction
(P1) `compaction.type` SHALL accept `summary` (default behavior of this spec), `native` and `auto` (default). With `auto` or `native`, when the Turn model's adapter advertises `capabilities.native_compaction` (for example OpenAI Responses server-side context compaction with an encrypted checkpoint), compaction SHALL call the provider's compaction instead of generating a summary, store the opaque checkpoint on the compaction record, and still start a new Context Epoch and record `session.compaction.completed.1` with `type: "native"`. The durable task-state record SHALL be maintained regardless of type. A later Turn on a model without native compaction SHALL fall back to summary compaction of the stored history. `native` on a model without the capability SHALL fail the compaction with `NativeCompactionUnavailable`.

#### Scenario: Responses API compaction
- **WHEN** a Session on an OpenAI Responses model with native compaction crosses the threshold with `compaction.type: auto`
- **THEN** the provider compacts the context, no summary model call is made, and the completed event records `type: "native"` and the token savings
