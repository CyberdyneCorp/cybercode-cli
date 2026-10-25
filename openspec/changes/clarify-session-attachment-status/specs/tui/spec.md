## ADDED Requirements

### Requirement: Explicit TUI attachment lifecycle
(P1) The TUI SHALL register an owner-authenticated Session/Location-bound interactive attachment independently of native-computer support, using the existing 45-second lease and a 15-second renewal interval. It SHALL release only its own lease on normal exit or context change with bounded teardown, fence late replies and recover from expired/restarted-server leases. This lifecycle SHALL preserve history, ongoing server work, existing approval rules and native Stop behavior; registration SHALL create no desktop grant.

#### Scenario: Closing and resuming the CLI
- **WHEN** an attached TUI closes normally while server work remains
- **THEN** its lease SHALL be released without deleting history or implicitly stopping that work
- **AND** another live client's lease SHALL remain independent

#### Scenario: Crash or context change
- **WHEN** a TUI crashes or switches Session/Location during attachment registration
- **THEN** abandoned leases SHALL expire within the existing 45-second TTL and stale replies SHALL NOT attach or retarget the new context

### Requirement: Distinct attachment and execution labels
(P1) The sidebar and agent directory SHALL distinguish `attached` clients, `background` running work and `saved` inactive Sessions using independent attachment and execution observations, preserve waiting state and provenance, and explicitly label unknown/stale attachment data. Labels SHALL describe reported registered clients and SHALL NOT imply human attention. Discovery SHALL perform no stop, delete, attach or messaging action.

#### Scenario: Attached idle versus saved inactive
- **WHEN** a live attached Session is idle and a managed saved Session reports no attached client and is idle
- **THEN** the UI SHALL distinguish `attached · idle` from `saved · inactive` and explain that absence means no registered client reported

#### Scenario: Background and unknown activity
- **WHEN** a running Session reports no attached client or its attachment field is unavailable
- **THEN** the former SHALL be distinguishable as background work and the latter SHALL retain explicit attachment uncertainty
- **AND** source provenance and the directory overflow link SHALL remain visible in supported compact layouts
