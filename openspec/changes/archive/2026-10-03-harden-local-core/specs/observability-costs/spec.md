## ADDED Requirements

### Requirement: Budget reservations and unknown usage
(P0) Budgets SHALL default to `enforcement: soft`: they block new calls at the recorded limit and disclose possible in-flight overshoot. Optional `enforcement: reserved` SHALL atomically reserve conservative input and capped output cost before dispatch across every applicable scope, including descendants, title, evaluator, advisor and compaction calls. Reservation failure SHALL block dispatch. Reserved USD budgets SHALL reject unknown pricing or an unbounded provider charge with `BudgetUnpricedError` until an explicit price bound or token budget is supplied. Actual usage SHALL reconcile reservations once by request ID; missing usage after interruption SHALL retain the reservation as uncertain until reconciled or explicitly released. Reservations bound scheduling estimates, not a provider billing guarantee. UI and reports SHALL disclose estimated charges, uncertainty and any overrun.

#### Scenario: Parallel agents compete for remaining funds
- **WHEN** two calls each require a $0.75 reservation and $1.00 remains in their shared budget
- **THEN** only one call reserves funds and starts; the other waits or returns budget exhausted

## MODIFIED Requirements

### Requirement: Per-step usage accounting
(P0) The system SHALL record for every provider step the token classes `input` (excluding cache), `output` (excluding reasoning), `reasoning`, `cache_read` and `cache_write`, together with provider, model, variant, duration and finish reason. These SHALL be stored on the assistant message and as a durable `usage.recorded.1` event.

#### Scenario: Usage stored
- **WHEN** a Turn completes with 12,000 input, 800 output and 9,000 cache-read tokens
- **THEN** the assistant message records each class separately and a `usage.recorded.1` event is appended

### Requirement: Cost computation
(P0) The system SHALL compute step cost as each token class multiplied by the model's per-million price from the catalog, with reasoning billed at the output price unless the catalog gives a reasoning price. Context-tier pricing SHALL apply when input exceeds a tier threshold. Provider-reported cost SHALL override computed cost when present. Models with unknown pricing SHALL record `cost: null` and be flagged `unpriced`, never shown as zero.

#### Scenario: Unpriced local model
- **WHEN** a local Ollama model without pricing is used
- **THEN** usage is recorded and cost is shown as `—` (unpriced), not `$0.00`

### Requirement: Cost rollups
(P0) The system SHALL aggregate usage and cost per session (including descendants), and (P2) per workflow run, goal, loop and routine run, per agent, per model and per plugin. Rollups SHALL be exposed at `GET /api/v1/usage?scope=<session|run|goal|loop|routine|project>&id=...`.

#### Scenario: Workflow run cost
- **WHEN** workflow run `run_7` spawned 12 agents
- **THEN** `GET /api/v1/usage?scope=run&id=run_7` returns tokens and cost summed across all 12 agents plus the orchestrator

### Requirement: Budgets
(P0) The system SHALL support budgets in config `telemetry.budgets` and per invocation: `session_usd`, `run_usd`, `goal_usd`, `loop_usd`, `daily_usd`, and token equivalents (`*_tokens`). It SHALL emit a warning at 80% and stop further provider turns for the budgeted scope at 100%, publishing `budget.exceeded.1`.

#### Scenario: Run budget stops workflow
- **WHEN** a workflow run started with `--budget-usd 5` reaches $5.00
- **THEN** no new agent Turns start, in-flight Turns finish, and the run ends with status `budget_exceeded`

#### Scenario: Warning at 80%
- **WHEN** a session with `session_usd: 2` reaches $1.60
- **THEN** the user sees a budget warning once
