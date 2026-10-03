## MODIFIED Requirements

### Requirement: Budgets
(P0) A Budget SHALL be the object `{ max_turns?, max_tokens?, max_cost_usd?, max_wall_seconds?, enforcement?: "soft" | "reserved" }`. Every budgeted scope (Session, Workflow Run, Goal, Loop, Routine run, Team, `exec` run) SHALL accept exactly this object, extended only by scope-specific fields documented in the owning spec (for example workflow `max_agents`). Wherever a budget is accepted on the command line, the flags SHALL be `--max-turns <n>`, `--max-tokens <n>`, `--max-cost <usd>` and `--timeout <duration>`. Per-scope defaults SHALL come from `budgets.<scope>` in config with scopes `session`, `run`, `goal`, `loop`, `routine`, `team` and `daily` (a per-machine daily cap across all scopes). The system SHALL emit a warning at 80% and stop further provider Turns for the budgeted scope at 100%, publishing `budget.exceeded.1`.

#### Scenario: Run budget stops workflow
- **WHEN** a workflow run started with `--max-cost 5` reaches $5.00
- **THEN** no new agent Turns start, in-flight Turns finish, and the run ends with status `budget_exceeded`

#### Scenario: Warning at 80%
- **WHEN** a session with `budgets.session.max_cost_usd: 2` reaches $1.60
- **THEN** the user sees a budget warning once

### Requirement: Structured logs
(P0) The system SHALL write structured JSON-lines logs to `<data>/log/cyber-<YYYY-MM-DD>.log`, one file per day, keeping 14 days of files (the contract shared with `cli-commands` → Logging destination), at level `info` by default (`--log-level`, `CYBER_LOG_LEVEL`). `--print-logs` SHALL additionally mirror logs to stderr. Each line SHALL include timestamp, level, component, and when applicable session id and request id.

#### Scenario: Rotation
- **WHEN** a log file is older than 14 days at startup
- **THEN** it is deleted
