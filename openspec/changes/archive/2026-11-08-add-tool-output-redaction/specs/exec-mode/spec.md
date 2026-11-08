## ADDED Requirements

### Requirement: Redaction notes in exec output
(P1) A `tool_result` line of `--format stream-json` SHALL carry the settlement's `redactions` when it hid anything (`tool-registry` → Redaction visibility). In text mode without `--quiet`, `cyber exec` SHALL print `<tool>: <n> secret(s) hidden from the model` to stderr.

#### Scenario: Stream line
- **WHEN** a `bash` call settles with two values hidden
- **THEN** its `tool_result` line has `"redactions": {"count": 2, "kinds": […]}`
