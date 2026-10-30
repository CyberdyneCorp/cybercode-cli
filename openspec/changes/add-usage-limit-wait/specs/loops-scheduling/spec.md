## ADDED Requirements

### Requirement: Usage limit waits in loop iterations
(P2) A Session with a loop iteration in flight SHALL count as unattended for `session-runtime` → Usage limit wait, so the iteration waits for the reset and continues instead of failing. The wait SHALL count as wall time of the iteration and against the loop's `max_wall_seconds`; when the planned resume falls at or past the loop's wall deadline the system SHALL NOT wait and SHALL stop the loop with status `budget_exceeded` naming `max_wall_seconds`. Ticks that come due during the wait SHALL follow the loop's overlap policy, as for any long iteration.

#### Scenario: Iteration waits for the reset
- **WHEN** a `/loop 30m` iteration hits a usage limit that resets in 10 minutes
- **THEN** the iteration resumes after the reset and ends normally, and the next tick follows the overlap policy
