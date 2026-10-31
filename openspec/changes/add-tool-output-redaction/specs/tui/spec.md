## ADDED Requirements

### Requirement: Redaction notice on tool cards
(P1) A tool card whose settlement hid secrets SHALL show one line under its header, `⊘ <n> secret(s) hidden from the model`, in the warning color (`tool-registry` → Redaction visibility). The output SHALL show the markers as they are.

#### Scenario: Two secrets hidden
- **WHEN** a settled `bash` call has `redactions.count: 2`
- **THEN** its card shows `⊘ 2 secrets hidden from the model`
