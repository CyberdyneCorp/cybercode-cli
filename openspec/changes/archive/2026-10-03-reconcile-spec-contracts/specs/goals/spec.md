## MODIFIED Requirements

### Requirement: Goal budgets
(P2) Each Goal SHALL enforce a Budget (`observability-costs`) with `max_turns` defaulting to `budgets.goal.max_turns` (50) and optional `max_tokens`, `max_cost_usd` and `max_wall_seconds`, counted from activation and including subagent costs. When exceeded, the Goal SHALL become `budget_exceeded`, continuation SHALL stop and the user SHALL be notified with spend figures.

#### Scenario: Turn budget
- **WHEN** a Goal with default budget reaches 50 Turns without being met
- **THEN** it becomes `budget_exceeded` and the Session stops continuing
