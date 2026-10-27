## ADDED Requirements

### Requirement: Root prompt structured result lifecycle
(P1) An ordinary root prompt SHALL accept a validated per-prompt JSON Schema format without creating a child Session. Its `structured_output` finalizer SHALL store only a schema-valid value on the matching input/assistant attempt. It SHALL allow at most two validation corrections after the initial submission within ordinary budgets, then return `StructuredOutputError`. Missing finalization SHALL NOT be a successful structured result. Attempt state SHALL survive compaction/restart and remain separate from other queued prompts and existing child result contracts.

#### Scenario: Third invalid submission
- **WHEN** a root prompt returns three invalid structured submissions
- **THEN** it ends with `StructuredOutputError` without a fourth attempt or a successful partial JSON result

#### Scenario: Restart between queued schemas
- **WHEN** a Session restarts after completing one schema-governed prompt and before promoting another with a different schema
- **THEN** each retained input has its own schema, retry count and result and the second cannot reuse the first's successful value
