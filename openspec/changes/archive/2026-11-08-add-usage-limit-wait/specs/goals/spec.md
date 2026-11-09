## ADDED Requirements

### Requirement: Usage limit waits under a Goal
(P2) A Session with an active Goal SHALL count as unattended for `session-runtime` → Usage limit wait. A usage limit wait SHALL NOT pause the Goal; the waited Turn's retry SHALL be evaluated like any other Turn. The wait SHALL count as wall time against the Goal's `max_wall_seconds`, and when the planned resume falls at or past the Goal's wall deadline the system SHALL NOT wait and SHALL end the Goal as `budget_exceeded` naming `max_wall_seconds`. An interrupt during the wait SHALL pause the Goal as an interrupted Turn does.

#### Scenario: Goal waits through a usage limit
- **WHEN** an active Goal's Turn hits a usage limit that resets in 20 minutes and the Goal has no wall budget
- **THEN** the Goal stays active, the Turn runs again after the reset and the evaluator then runs on it

#### Scenario: Goal wall budget ends first
- **WHEN** a Goal with `max_wall_seconds: 600` activated 5 minutes ago hits a usage limit that resets in 20 minutes
- **THEN** the Goal ends `budget_exceeded` with a `max_wall_seconds` reason and no wait is recorded
