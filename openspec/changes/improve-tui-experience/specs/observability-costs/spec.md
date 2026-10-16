## ADDED Requirements

### Requirement: Usage step ledger
(P1) The system SHALL keep a queryable ledger with one row per provider step and per auxiliary model call (title, compaction, recap, evaluator), maintained in the same transaction as the durable `session.step.ended.1`, `session.step.failed.1` and `usage.recorded.1` events. Each row SHALL hold: Session ID, root Session ID, parent Session ID, Workflow Run ID, agent, purpose (`turn` or the auxiliary purpose), provider, model, `started_ms`, `ended_ms`, the five token classes, prompt tokens (input + cache read + cache write), cost (or null when unpriced) and the Location directory. A data migration SHALL backfill the ledger from existing events. Pruning under Usage data retention SHALL fold rows into daily aggregates per UTC day, model, agent and root Session, with the number of steps and the maximum prompt tokens of the day.

#### Scenario: Backfill on upgrade
- **WHEN** a database with 40 Sessions of stored step events is opened by a version with the ledger
- **THEN** the migration creates one ledger row per recorded step, and the session-scope usage totals equal the ledger sums

### Requirement: Session usage breakdown
(P1) `GET /api/v1/usage?scope=session&id=<id>` SHALL also return, for the Session and its descendants:
- `by_model`: one entry per provider and model, with the five token classes, cost (null when unpriced), unpriced steps and step count
- `api_duration_ms`: the sum of step durations (`ended_ms - started_ms`) across the tree
- `wall_duration_ms`: from the first admitted prompt to the last recorded activity, or to now while a Drain runs
- `lines_added` and `lines_removed`: summed over the Session's per-message diffs, and null when the snapshot history is unavailable
- `budgets`: each Budget that applies to the Session (its own and `budgets.daily`), with limit, used amount and, for the daily budget, the next reset time

Existing fields SHALL keep their meaning.

#### Scenario: Breakdown includes subagents
- **WHEN** a Session used `claude-opus-5-5` and its subagent used `claude-haiku-5-5`
- **THEN** `by_model` has two entries, and their costs add up to `total.cost`

### Requirement: Usage statistics API
(P1) The server SHALL provide `GET /api/v1/usage/stats?period=<all|Nd>&directory=<path>&tz=<IANA zone>`, where `Nd` is 1 to 371 days (`7d`, `30d`). `period` defaults to `all`. Without `directory` it covers every Location; with it, only Sessions of that Location's project. `tz` defaults to the server's zone and sets the local day boundaries. The response SHALL contain:
- `days`: one entry per local day in the period (at most the last 371 days for `all`), with tokens by class, cost, Turns and Sessions
- `totals`: tokens by class, cost, unpriced steps, Sessions, Turns
- `favorite_model` (most tokens), `longest_session` (ID, title and wall duration), `active_days` and `period_days`, `longest_streak` and `current_streak` in days, and `most_active_day`
- `models`: per model, tokens by class, cost, Turns, Sessions and share of tokens

Days with no Turns SHALL be returned with zero values. Periods older than the retention window SHALL be served from daily aggregates, with UTC-day boundaries, flagged `approximate_days: true`. `cyber stats` and the TUI Stats tab SHALL both render this response.

#### Scenario: Streaks
- **WHEN** the user had Turns on each of the last 14 local days and not on the day before
- **THEN** `current_streak` is 14

#### Scenario: Project scope
- **WHEN** the request passes `directory=/repo/a`
- **THEN** only Sessions of the project of `/repo/a` are counted

### Requirement: Usage insights
(P1) The server SHALL provide `GET /api/v1/usage/insights?window=24h|7d&directory=<path>`, computed from the usage step ledger. It SHALL return `basis` (`cost` when every step in the window is priced, else `tokens`), the window total, and factors with their `share` (0–1) of the window total. Factors may overlap:
- `subagent_heavy`: usage of root Session trees in which descendants account for more than 50% of the tree's usage
- `long_running`: usage of Sessions whose first and last step in the window are at least 8 hours apart
- `large_context`: usage of steps whose prompt tokens exceed 150,000
- (P2) `workflow`: usage of Sessions that belong to a Workflow Run

It SHALL also return `subagents`: the share of the window total per subagent group, where a group is the child Session's agent name, `workflow` for Workflow Run agents and `fork` for forked Sessions. The response SHALL state that it covers only Sessions stored by this server.

#### Scenario: Large context share
- **WHEN** 84% of the last 24 hours of cost came from steps with more than 150,000 prompt tokens
- **THEN** the `large_context` factor has `share` 0.84 and `basis` `cost`

#### Scenario: Unpriced usage changes the basis
- **WHEN** the window contains steps of an unpriced local model
- **THEN** `basis` is `tokens` and every share is a share of tokens

### Requirement: Provider-reported limits
(P2) When a provider reports account usage windows (for example rate-limit or plan-usage response headers, or a provider usage endpoint), the provider adapter SHALL record the latest reading per provider and window: label, percent used, reset time and observation time. These readings SHALL be exposed at `GET /api/v1/usage/limits`. The system SHALL never derive, estimate or extrapolate a provider limit from local usage, and providers without such reports SHALL be absent from the response.

#### Scenario: Provider without limit reporting
- **WHEN** only an OpenAI-compatible local gateway is configured and it sends no usage window headers
- **THEN** `GET /api/v1/usage/limits` returns an empty list

## MODIFIED Requirements

### Requirement: Usage commands
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), `/usage` (the current Session tree's breakdown, configured Budgets, provider-reported limits and contributing factors for the last 24 hours or 7 days) and `/stats` (activity, streaks and per-model totals for all time, the last 7 days and the last 30 days). In the TUI these SHALL open the matching tab of the Settings panel. It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--project <id>] [--plugins] [--format json]`.

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

#### Scenario: Cost opens the Usage tab
- **WHEN** the user types `/cost` in the TUI
- **THEN** the Settings panel opens on the Usage tab at the Session section
