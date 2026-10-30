## ADDED Requirements

### Requirement: Usage limit waits of workflow agents
(P2) A workflow agent's Session SHALL count as unattended for `session-runtime` → Usage limit wait. The waiting agent SHALL keep its `workflows.max_concurrent` slot and the run its run slot, because the `agent()` call is still executing and a usage limit belongs to the account, so another agent given the slot on the same provider would hit the same limit. The wait SHALL count against the run's `max_wall_seconds`; when the planned resume falls at or past the run's wall deadline the agent SHALL NOT wait and SHALL fail with `BudgetExceeded: max_wall_seconds`. Pausing or stopping the run SHALL interrupt the waiting agent like a running one. After a restart a workflow agent's wait SHALL be abandoned and the run's own recovery SHALL apply.

#### Scenario: Agent waits inside a run
- **WHEN** one agent of a running run hits a usage limit that resets in 5 minutes
- **THEN** the agent keeps its slot, resumes after the reset and its `agent()` call settles with its result

#### Scenario: Run wall budget ends first
- **WHEN** a run with 2 minutes of `max_wall_seconds` left has an agent hit a usage limit that resets in 5 minutes
- **THEN** the agent fails with `BudgetExceeded: max_wall_seconds` without waiting
