## MODIFIED Requirements

### Requirement: Compaction hooks
(P1) The system SHALL run `PreCompact` hooks before the summary request and `PostCompact` hooks after completion, as defined by the hooks capability. A `PreCompact` decision's `additional_context` SHALL be appended to the summary instructions, and a `deny` SHALL block an automatic compaction (a manual `/compact` reports the reason and stops).

#### Scenario: Hook adds context
- **WHEN** a `PreCompact` hook returns `additional_context: "keep the TODO list"`
- **THEN** the summary request includes that text
