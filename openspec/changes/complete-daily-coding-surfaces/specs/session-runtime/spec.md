## ADDED Requirements

### Requirement: Root prompt structured result lifecycle
(P1) An ordinary root prompt SHALL accept a validated per-prompt JSON Schema format without creating a child Session. Its `structured_output` finalizer SHALL store only a schema-valid value on the matching input/assistant attempt. It SHALL allow at most two validation corrections after the initial submission within ordinary budgets, then return `StructuredOutputError`. Missing finalization SHALL NOT be a successful structured result. Attempt state SHALL survive compaction/restart and remain separate from other queued prompts and existing child result contracts. The schema SHALL be an object schema of at most 64 KiB and 64 levels with local references only, checked before admission; a retry with the same ID and another schema SHALL be refused as a different prompt. A Turn that ends without the finalizer SHALL count as a failed submission and add a reminder before the next attempt. The terminal outcome SHALL be recorded as `session.structured_output.settled.1` with the input's `message_id`, `value` or `error`, and `attempts`, and kept on the input's inbox row as `structured`.

#### Scenario: Third invalid submission
- **WHEN** a root prompt returns three invalid structured submissions
- **THEN** it ends with `StructuredOutputError` without a fourth attempt or a successful partial JSON result

#### Scenario: Answer without the finalizer
- **WHEN** a schema-governed root prompt's first Turn ends with prose and no `structured_output` call
- **THEN** a reminder is added, the next Turn's valid call is the result, and the outcome records `attempts: 2`

#### Scenario: Restart between queued schemas
- **WHEN** a Session restarts after completing one schema-governed prompt and before promoting another with a different schema
- **THEN** each retained input has its own schema, retry count and result and the second cannot reuse the first's successful value
