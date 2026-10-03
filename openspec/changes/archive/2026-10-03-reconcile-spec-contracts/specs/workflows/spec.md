## MODIFIED Requirements

### Requirement: Run budgets
(P2) A run SHALL accept the Budget object defined by `observability-costs` (`max_turns`, `max_tokens`, `max_cost_usd`, `max_wall_seconds` default 14400, `enforcement`) plus the workflow-specific `max_agents` (default 200), from `meta`, invocation options or `budgets.run`. When a budget would be exceeded, new `agent()` calls SHALL reject with `BudgetExceeded` naming the budget, in-flight agents SHALL finish, and `ctx.budget` SHALL expose `{ spent_tokens, spent_cost_usd, agents_started, remaining }` to the script at any time. Cost and token dispatch SHALL obey the soft/reserved enforcement and uncertain usage rules in observability-costs, including checks before each child provider Turn. A halted budgeted run SHALL enter `budget_exceeded`.

#### Scenario: Cost budget reached
- **WHEN** a run with `max_cost_usd: 5` has spent $5.02
- **THEN** the next `agent()` call rejects with `BudgetExceeded: max_cost_usd` and the script may return a partial result

### Requirement: Workflow CLI
(P2) The CLI SHALL provide `cyber workflows list` (saved and bundled), `run <name|file> [--args JSON] [--max-cost N] [--max-turns N] [--max-tokens N] [--timeout D] [--max-agents N] [--runner <id>] [--wait]`, `runs` (recent runs), `show <run_id>`, `pause <run_id>`, `resume <run_id>`, `stop <run_id>` and `logs <run_id>`. `run --wait` SHALL stream progress and exit 0 on `completed`, 1 on `failed`, 4 on `BudgetExceeded`, and 130 when interrupted.

#### Scenario: CI usage
- **WHEN** CI runs `cyber workflows run audit --wait --max-cost 10`
- **THEN** the command blocks until the run ends and exits with the mapped code
