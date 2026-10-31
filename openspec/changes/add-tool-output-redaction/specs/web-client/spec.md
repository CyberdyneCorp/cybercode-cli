## ADDED Requirements

### Requirement: Redaction badge on tool results
(P1) The dashboard SHALL show an amber badge `<n> secret(s) hidden from the model` in the summary of a tool call whose settlement hid secrets (`tool-registry` → Redaction visibility), and no badge otherwise.

#### Scenario: One secret hidden
- **WHEN** a tool call has `redactions.count: 1`
- **THEN** its summary shows the badge `1 secret hidden from the model`
