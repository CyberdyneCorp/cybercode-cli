## ADDED Requirements

### Requirement: Reachable Session attachment observation
(P1) Authorized reachable Session records SHALL expose an additive optional nullable `interactiveAttached` observation independently of execution state. True SHALL require a live registered exact Session/Location interactive attachment; false SHALL mean managed tracking reports no live registered client; absent/null SHALL mean unknown. Unmanaged, older or unsupported sources SHALL NOT be treated as detached. Observation SHALL disclose no attachment identifiers or grant authority and SHALL NOT treat broad observer streams or hub online state as proof of client attachment.

#### Scenario: Idle attached client and retained history
- **WHEN** one idle Session has a live registered interactive client and another managed idle Session has none
- **THEN** their execution state SHALL remain idle and their attachment observations SHALL be true and false respectively
- **AND** both histories SHALL remain available and neither Session SHALL be deleted or stopped by observing the listing

#### Scenario: Expiry and multiple clients
- **WHEN** one client closes or expires while another exact-scope lease remains live
- **THEN** the Session SHALL remain attached until its last live registered client is released or expires

#### Scenario: Compatibility and remote uncertainty
- **WHEN** an older peer omits attachment data, a hub supplies only online state, or local tracking is unmanaged
- **THEN** attachment SHALL remain unknown and no absence of a terminal SHALL be inferred
