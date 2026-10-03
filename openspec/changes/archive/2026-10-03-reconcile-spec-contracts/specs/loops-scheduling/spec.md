## MODIFIED Requirements

### Requirement: Budgets for loops
(P2) A Loop or schedule SHALL accept a Budget (`observability-costs`) applied across iterations, defaulted by `budgets.loop`, with the standard flags (`--max-cost`, `--max-tokens`, `--max-turns`, `--timeout`); when exceeded, it SHALL stop with status `budget_exceeded` and notify.

#### Scenario: Loop budget
- **WHEN** a loop with `--max-cost 2` has spent $2.01 across iterations
- **THEN** it stops and the user is notified
