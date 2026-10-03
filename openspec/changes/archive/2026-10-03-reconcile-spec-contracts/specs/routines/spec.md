## MODIFIED Requirements

### Requirement: Runs as sessions
(P3) Each run SHALL have an ID with prefix `rrn_` (distinct from Workflow Run `run_`) and SHALL create a Session linked to it on the target Runner, with the Routine's agent, model, mode ceiling and Environment. For kind `goal` it SHALL set the goal condition before the first Turn. For kind `workflow` it SHALL start the named Workflow Run with the rendered args and link both IDs.

#### Scenario: Goal routine
- **WHEN** a Routine of kind `goal` with condition "all lint errors fixed" fires
- **THEN** the Session starts with that active goal and continues until the evaluator reports it met or impossible, or the budget is exhausted

### Requirement: Budgets
(P3) Each Routine SHALL have a per-run Budget (`observability-costs`) with defaults `max_turns` 200, `max_cost_usd` 5 and `max_wall_seconds` 7200, overridable per Routine and defaulted by `budgets.routine`. A run exceeding a budget SHALL be interrupted at the next Safe Boundary with status `budget_exceeded` and SHALL notify the owner.

#### Scenario: Cost cap
- **WHEN** a run reaches its `max_cost_usd`
- **THEN** it stops at the next Safe Boundary with status `budget_exceeded`
