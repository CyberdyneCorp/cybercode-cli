## MODIFIED Requirements

### Requirement: Corrupt snapshot handling
(P0) The system SHALL fail a Turn with `ContextSnapshotDecodeError` naming the Session, without contacting the provider, when the stored Context Snapshot itself cannot be decoded, and SHALL offer `cyber sessions repair-context <id>` to start a new epoch.

#### Scenario: Repair a corrupted snapshot
- **WHEN** the user runs `cyber sessions repair-context ses_1`
- **THEN** a new Context Epoch is created from current sources and the Session can continue
