## MODIFIED Requirements

### Requirement: Budgets
(P0) A Budget SHALL be the object `{ max_turns?, max_tokens?, max_cost_usd?, max_wall_seconds?, enforcement?: "soft" | "reserved" }`. Every budgeted scope (Session, Workflow Run, Goal, Loop, Routine run, Team, `exec` run) SHALL accept exactly this object, extended only by scope-specific fields documented in the owning spec (for example workflow `max_agents`). Wherever a budget is accepted on the command line, the flags SHALL be `--max-turns <n>`, `--max-tokens <n>`, `--max-cost <usd>` and `--timeout <duration>`. Per-scope defaults SHALL come from `budgets.<scope>` in config with scopes `session`, `run`, `goal`, `loop`, `routine`, `team` and `daily` (a per-machine daily cap across all scopes). The system SHALL emit a warning at 80% and stop further provider Turns for the budgeted scope at 100%, publishing `budget.exceeded.1`.

#### Scenario: Run budget stops workflow
- **WHEN** a workflow run started with `--max-cost 5` reaches $5.00
- **THEN** no new agent Turns start, in-flight Turns finish, and the run ends with status `budget_exceeded`

#### Scenario: Warning at 80%
- **WHEN** a session with `budgets.session.max_cost_usd: 2` reaches $1.60
- **THEN** the user sees a budget warning once


#### Scenario: Durable soft Session subtree budget
- **WHEN** a Session declares a soft Budget explicitly or through trusted budgets.session configuration
- **THEN** every visible or hidden provider dispatch in the Session and its descendants SHALL check all ancestor scopes against durable recorded usage
- **AND** reaching a token, cost, completed Turn or wall-time limit SHALL refuse new provider calls and publish budget.exceeded.1 once per limit, while already in-flight calls may settle with disclosed overshoot
- **AND** an 80 percent warning SHALL be durable and published once per limit
- **AND** restart SHALL preserve the Budget and its activation time, descendant deletion SHALL retain its billed turns and usage, and forked history SHALL not copy spending
- **AND** uncertain historical attribution SHALL refuse budgeted dispatch rather than silently treating missing charges as zero

#### Scenario: Budget validation and untrusted widening
- **WHEN** layered configuration or a Session request supplies malformed or unknown Budget fields
- **THEN** validation SHALL reject the malformed object with a scoped diagnostic
- **AND** untrusted project budget overrides SHALL remain inactive until workspace approval
- **AND** reserved enforcement SHALL never be interpreted as soft enforcement when conservative reservation support is unavailable
