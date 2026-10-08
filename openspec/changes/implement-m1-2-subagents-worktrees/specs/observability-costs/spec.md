## MODIFIED Requirements

### Requirement: Cost rollups
(P0) The system SHALL aggregate usage and cost per session (including descendants), and (P2) per workflow run, goal, loop and routine run, per agent, per model and per plugin. Rollups SHALL be exposed at `GET /api/v1/usage?scope=<session|run|goal|loop|routine|project>&id=...`.

#### Scenario: Workflow run cost
- **WHEN** workflow run `run_7` spawned 12 agents
- **THEN** `GET /api/v1/usage?scope=run&id=run_7` returns tokens and cost summed across all 12 agents plus the orchestrator

#### Scenario: Background Job attempt subtree usage
- **WHEN** a background subagent Job settles after nested provider usage
- **THEN** its recorded tokens and known cost SHALL include the child and descendant charges since the attempt baseline, including charges retained after descendant deletion
- **AND** resuming SHALL capture a durable subtree baseline so previous attempts are not billed again
- **AND** incomplete historical baselines or unpriced nested calls SHALL disclose uncertainty rather than inventing a complete zero cost

#### Scenario: Durable descendant token classes
- **WHEN** descendant visible or hidden provider usage is billed
- **THEN** surviving ancestors SHALL retain separate input, output, reasoning, cache-read and cache-write token counts independently of their own usage
- **AND** deleting a descendant SHALL preserve its token-class receipts on surviving ancestors
- **AND** migration SHALL backfill surviving source evidence and disclose incomplete token-class attribution when historical source evidence is missing
- **AND** Session detail/list and generated SDK contracts SHALL expose the known token classes and their completeness separately from combined token/cost attribution

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

#### Scenario: Budget refusal during preparation or retry
- **WHEN** another descendant exhausts a shared scope during model resolution or before a provider retry
- **THEN** the refused dispatch SHALL retain BudgetExceededError instead of a generic compaction or provider invalid-request failure
- **AND** a refused visible step SHALL settle durably before exactly one live budget error is published
- **AND** a refused retry SHALL not reach the underlying provider adapter
- **AND** storage failures at the same boundary SHALL retain their runtime storage error classification

#### Scenario: Ordinary exec subtree accounting
- **WHEN** an ordinary exec run submits a prompt to a new or reused Session
- **THEN** it SHALL capture a durable own/descendant baseline before submission and report spending since that baseline without rebilling prior history
- **AND** token and cost monitoring SHALL observe hidden and descendant usage even without a parent step event
- **AND** budgeted execution SHALL refuse missing or incomplete descendant accounting before prompt dispatch
- **AND** final reporting SHALL preserve separate own and descendant totals, all five own token classes and attribution completeness
- **AND** client-side soft monitoring SHALL disclose in-flight overshoot and SHALL NOT claim that ordinary conversation interruption proves descendant task cancellation

### Requirement: Usage commands
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), and `/usage` (today, last 7 and 30 days for this machine). It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--project <id>] [--plugins] [--format json]`.

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

#### Scenario: Session cost snapshot
- **WHEN** a TUI user opens `/cost`
- **THEN** it SHALL refresh the Session snapshot and show own, descendant and combined input, output, reasoning, cache-read and cache-write counts
- **AND** cache hit rate SHALL be cache-read tokens divided by all prompt tokens (input plus cache read plus cache write), with no-token and unknown attribution states shown explicitly
- **AND** unpriced calls and incomplete historical attribution SHALL prevent known priced spending from being presented as a complete total
- **AND** the user SHALL be able to refresh the open snapshot and dismiss it without sending a provider prompt
