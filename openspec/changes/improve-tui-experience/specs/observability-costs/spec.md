## ADDED Requirements

### Requirement: Usage step ledger
(P1) The system SHALL keep a queryable ledger with one row per provider step and per auxiliary model call (title, compaction, recap, evaluator), maintained in the same transaction as the durable `session.step.ended.1`, `session.step.failed.1` and `usage.recorded.1` events. Each row SHALL hold: Session ID, root Session ID, parent Session ID, Workflow Run ID, agent, purpose (`turn` or the auxiliary purpose), provider, model, `started_ms`, `ended_ms`, the five token classes, prompt tokens (input + cache read + cache write), cost (or null when unpriced) and the Location directory. A failed step SHALL have a row with no tokens and zero cost, and rows SHALL be kept when their Session is deleted, like retained descendant charges. A data migration SHALL backfill the ledger from existing events in resumable batches of 5,000 events, and the usage responses SHALL report `backfilling: true` until it finishes. Pruning under Usage data retention SHALL fold rows into daily aggregates per UTC day, model, agent and root Session, with the number of steps and the maximum prompt tokens of the day.

#### Scenario: Backfill on upgrade
- **WHEN** a database with 40 Sessions of stored step events is opened by a version with the ledger
- **THEN** the migration creates one ledger row per recorded step, and the session-scope usage totals equal the ledger sums

### Requirement: Session usage breakdown
(P1) `GET /api/v1/usage?scope=session&id=<id>` SHALL also return, for the Session and its descendants:
- `by_model`: one entry per provider and model, with the five token classes, cost (null when unpriced), unpriced steps and step count
- `api_duration_ms`: the sum of step durations (`ended_ms - started_ms`) across the tree
- `wall_duration_ms`: from the first admitted prompt to the last recorded activity, or to now while a Drain runs
- `lines_added` and `lines_removed`: summed over the Session's per-message diffs, and null when the snapshot history is unavailable
- `budgets`: each Budget that applies to the Session (its own and `budgets.daily`), with the limit and used amount of each cost, token and Turn limit and, for the daily budget, the next reset time (`reset_ms`, the next local midnight of the server) and the server's IANA `zone`; daily usage counts every Session stored by the server since local midnight
- `budgets_error`: present when the Session directory's configuration cannot be read for `budgets.daily` (a malformed budget or a file that does not parse); the daily budget is then left out and the rest of the report SHALL still be served
- `backfilling`: true while the ledger backfill runs, when `by_model` may be partial

Existing fields SHALL keep their meaning.

#### Scenario: Breakdown includes subagents
- **WHEN** a Session used `claude-opus-5-5` and its subagent used `claude-haiku-5-5`
- **THEN** `by_model` has two entries, and their costs add up to `total.cost`

### Requirement: Usage statistics API
(P1) The server SHALL provide `GET /api/v1/usage/stats?period=<all|Nd>&directory=<path>&tz=<IANA zone>`, where `Nd` is 1 to 371 days (`7d`, `30d`), the last N local days including today; any other `period` SHALL be refused with `400`. `period` defaults to `all`. Without `directory` it covers every Location; with it, only Sessions whose directory is inside that directory's project (its git root, or the directory itself outside git). `tz` defaults to the server's zone and sets the local day boundaries; a name that is unknown, or that is not a plausible IANA name (path syntax, control bytes, more than 64 characters), SHALL fall back to the server's zone without a lookup, and the response SHALL name the zone used. Turns are finished provider steps and Sessions are root Session trees; an active day is a day with at least one Turn. The response SHALL contain:
- `days`: one entry per local day in the period (at most the last 371 days for `all`), with tokens by class, cost, Turns and Sessions
- `totals`: tokens by class, cost, unpriced steps, Sessions, Turns
- `favorite_model` (most tokens), `longest_session` (root ID, title and the time from its first to its last call in the period), `active_days` and `period_days` (for `all`, from the first day with usage), `longest_streak` and `current_streak` in days (the current streak counts back from today, or from yesterday while today has no Turn yet), and `most_active_day` (the active day with the most tokens)
- `zone`, `since_ms` (the period's first local midnight, `null` for `all`) and `backfilling`
- `models`: per model, tokens by class, cost, Turns, Sessions and share of tokens

Days with no Turns SHALL be returned with zero values. Periods older than the retention window SHALL be served from daily aggregates, with UTC-day boundaries, flagged `approximate_days: true`. `cyber stats` and the TUI Stats tab SHALL both render this response.

#### Scenario: Streaks
- **WHEN** the user had Turns on each of the last 14 local days and not on the day before
- **THEN** `current_streak` is 14

#### Scenario: Today without a Turn yet
- **WHEN** the user had Turns on each of the 5 local days before today and none today
- **THEN** `current_streak` is 5

#### Scenario: Unsafe zone name
- **WHEN** the request passes `tz=../../etc/passwd`
- **THEN** no zone file is read, the days use the server's zone and `zone` names it

#### Scenario: Project scope
- **WHEN** the request passes `directory=/repo/a`
- **THEN** only Sessions of the project of `/repo/a` are counted

### Requirement: Usage insights
(P1) The server SHALL provide `GET /api/v1/usage/insights?window=24h|7d&directory=<path>`, computed from the usage step ledger. It SHALL return `basis` (`cost` when every step in the window is priced, else `tokens`), the window total, and factors with their `share` (0–1) of the window total. Factors may overlap:
- `subagent_heavy`: usage of root Session trees in which descendants account for more than 50% of the tree's usage
- `long_running`: usage of Sessions whose first and last step in the window are at least 8 hours apart
- `large_context`: usage of steps whose prompt tokens exceed 150,000
- (P2) `workflow`: usage of Sessions that belong to a Workflow Run

It SHALL also return `subagents`: the share of the window total per subagent group, where a group is the child Session's agent name, `workflow` for Workflow Run agents and `fork` for forked Sessions. The response SHALL state that it covers only Sessions stored by this server. `window` defaults to `24h` and any other value SHALL be refused with `400`; the window is the last 24 hours or 7 days before the request, counting finished calls that started in it. The response also carries `since_ms`, `until_ms`, `total_cost` (null when nothing priced ran), `total_tokens`, `steps`, `scope: "server"`, `note` and `backfilling`; each factor and group carries its `amount` in the basis unit, and each group its `kind` (`agent`, `workflow` or `fork`). An empty window has a zero total and zero shares. The `workflow` factor is reserved and not reported until P2.

#### Scenario: Large context share
- **WHEN** 84% of the last 24 hours of cost came from steps with more than 150,000 prompt tokens
- **THEN** the `large_context` factor has `share` 0.84 and `basis` `cost`

#### Scenario: Unpriced usage changes the basis
- **WHEN** the window contains steps of an unpriced local model
- **THEN** `basis` is `tokens` and every share is a share of tokens

### Requirement: Provider-reported limits
(P2) When a provider reports account usage windows (for example rate-limit or plan-usage response headers, or a provider usage endpoint), the provider adapter SHALL record the latest reading per provider and window: label, percent used, reset time and observation time. These readings SHALL be exposed at `GET /api/v1/usage/limits` as `{ limits: [{ provider, window, label, period, used_percent, resets_ms, observed_ms }] }`, ordered by provider then window, where `period` is `daily` for windows of up to a day (rate limits, a 5-hour plan window) and `weekly` for longer ones, `used_percent` is 0–100 and `resets_ms` is null when the provider did not say. The recognized reports are the plan-window headers `anthropic-ratelimit-unified-<window>-utilization`/`-reset` and `x-codex-<primary|secondary>-used-percent`/`-window-minutes`/`-reset-after-seconds`/`-reset-at`, and the rate-limit headers `anthropic-ratelimit-<class>-limit`/`-remaining`/`-reset` and `x-ratelimit-limit|remaining|reset-<class>`; they SHALL be read from every response, including rate-limited and failed ones. Readings are kept in memory since the server started, the latest per provider and window, at most 64; header values that are not finite, out of range or name an unsafe window SHALL be ignored. The system SHALL never derive, estimate or extrapolate a provider limit from local usage, and providers without such reports SHALL be absent from the response.

#### Scenario: Provider without limit reporting
- **WHEN** only an OpenAI-compatible local gateway is configured and it sends no usage window headers
- **THEN** `GET /api/v1/usage/limits` returns an empty list

#### Scenario: A later response replaces the reading
- **WHEN** one response reported the 5-hour window at 25% and a later one at 50%
- **THEN** `GET /api/v1/usage/limits` lists that window once, at 50%, with the later observation time

### Requirement: Machine stats API
(P1) `GET /api/v1/system` SHALL report the machine the server runs on for the request's Location: `hostname_label` (the OS name and version, never the host or user name), `cores_logical`, `cores_physical` (or `null`), `cpu_idle_percent` (`null` until two CPU readings exist), `load_avg` (1, 5 and 15 minutes; `null` where the OS has none), `memory_total` and `memory_available` in bytes, `disk` (`{mount, total, available}` in bytes for the mount that holds the Location, or `null`) and `sampled_at` in Unix milliseconds. The server SHALL keep one sampler and SHALL refresh only CPU usage, memory and the matched disk, at most every 2 seconds, serving the cached sample in between. It SHALL never enumerate processes. The route SHALL use the same authentication as the other routes.

#### Scenario: Cached sample
- **WHEN** two requests arrive less than 2 seconds apart
- **THEN** both return the same `sampled_at`, and a request after 2 seconds returns a new sample with `cpu_idle_percent` between 0 and 100

#### Scenario: First reading
- **WHEN** the first request after the server starts arrives
- **THEN** `cpu_idle_percent` is `null`

## MODIFIED Requirements

### Requirement: Context window meter
(P1) The system SHALL expose, per session, the current context usage (tokens in the last request versus the model context window and the auto-compaction threshold) via the session status API and the status line data. It SHALL update after each Turn. `GET /api/v1/sessions/:id` and each `session.usage` live event SHALL carry `context: { used_tokens, window, compact_at, utilization, measured, compacted }`, where `used_tokens` is the last finished step's prompt tokens (input + cache read + cache write), `window` the context window of the Session's current model (so switching model re-resolves it), `compact_at` the prompt size above which auto-compaction runs (the window minus the larger of the output allowance and the compaction buffer), and `utilization` their ratio. An unknown window SHALL make `window`, `compact_at` and `utilization` null rather than 0, disabled auto-compaction SHALL make `compact_at` null, and `measured` SHALL be false before the first step and after a compaction until the next step, when `used_tokens` is 0 and only the window and compaction point are known (`compacted` is then true after a compaction). The Session route SHALL report `context: null` only when the Session has no model.

#### Scenario: Status line shows context
- **WHEN** a custom status line script runs
- **THEN** it receives JSON including `context.used_tokens`, `context.window` and `context.compact_at`

#### Scenario: Meter after a step
- **WHEN** a step's request used 100 prompt tokens of a 200,000-token window with a 1,000-token output allowance and buffer
- **THEN** the Session route reports `context` as `{ used_tokens: 100, window: 200000, compact_at: 199000, utilization: 0.0005 }`

### Requirement: Usage commands
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), `/usage` (the current Session tree's breakdown, configured Budgets, provider-reported limits and contributing factors for the last 24 hours or 7 days) and `/stats` (activity, streaks and per-model totals for all time, the last 7 days and the last 30 days). In the TUI these SHALL open the matching tab of the Settings panel. It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--project <id>] [--plugins] [--format json]`.

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

#### Scenario: Cost opens the Usage tab
- **WHEN** the user types `/cost` in the TUI
- **THEN** the Settings panel opens on the Usage tab at the Session section
