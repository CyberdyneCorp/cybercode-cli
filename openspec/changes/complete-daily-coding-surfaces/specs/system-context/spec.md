## ADDED Requirements

### Requirement: Scope-aware rule inspection and loading
(P1) Path-scoped rules SHALL be bound to trusted definition identity, project root, successful authorized file operation and Context Epoch. Failed/denied operations SHALL NOT consume once-per-Epoch delivery. Context inspection SHALL report effective sources, origins, token estimates, pending updates and honest utilization through authenticated read-only API/TUI surfaces, using the same source model as request assembly. It SHALL NOT start inference, launch integrations or read an unauthorized source to produce a count. Existing `instructions` array configuration SHALL remain compatible. The additive `instruction_limits` object SHALL provide `max_file_bytes` default 32768 and `max_total_bytes` default 131072; invalid nonpositive or incompatible limits SHALL fail configuration validation before loading sources.

#### Scenario: Rule after a denied read
- **WHEN** a matching file read is denied and a later authorized edit succeeds in the same Epoch
- **THEN** the read does not expose or consume the rule and the successful edit delivers it once

#### Scenario: Inspection of an untrusted source
- **WHEN** an owner inspects context while project sources are withheld by trust policy
- **THEN** inspection reports their withheld state without loading their content or treating their estimated bytes as active context
