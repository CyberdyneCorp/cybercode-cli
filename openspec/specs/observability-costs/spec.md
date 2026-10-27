# observability-costs Specification

## Purpose
Users and organizations must see what agents cost and what they did, and must be able to cap spending, especially for unattended workflows, goals, loops and routines. This capability covers usage and cost accounting, budgets and spend limits, usage CLI views, OpenTelemetry export, structured logs and traces, privacy, and `cyber doctor` health checks. It draws on OpenCode v1 (per-step cost with cache tokens and tiered pricing, `stats`, logs), Claude Code (OpenTelemetry, `/cost`, status line, spend limits) and Codex (`/status`, `/usage`).

## Requirements

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
(P0) A Budget SHALL be the object `{ max_turns?, max_tokens?, max_cost_usd?, max_wall_seconds?, enforcement?: "soft" | "reserved" }`. Every budgeted scope (Session, Workflow Run, Goal, Loop, Routine run, Team, `exec` run) SHALL accept exactly this object, extended only by scope-specific fields documented in the owning spec (for example workflow `max_agents`). Wherever a budget is accepted on the command line, the flags SHALL be `--max-turns <n>`, `--max-tokens <n>`, `--max-cost <usd>` and `--timeout <duration>`. Per-scope defaults SHALL come from `budgets.<scope>` in config with scopes `session`, `run`, `goal`, `loop`, `routine`, `team` and `daily` (a per-machine daily cap across all scopes). The system SHALL emit a warning at 80% and stop further provider Turns for the budgeted scope at 100%, publishing `budget.exceeded.1`.

#### Scenario: Run budget stops workflow
- **WHEN** a workflow run started with `--max-cost 5` reaches $5.00
- **THEN** no new agent Turns start, in-flight Turns finish, and the run ends with status `budget_exceeded`

#### Scenario: Warning at 80%
- **WHEN** a session with `budgets.session.max_cost_usd: 2` reaches $1.60
- **THEN** the user sees a budget warning once

### Requirement: Organization spend limits
(P3) When signed in with a Cyber Account whose organization policy defines spend limits (per user per day, week or month), the system SHALL enforce the stricter of the local and org limits. It SHALL report usage for hosted features to the org service, and SHALL treat an unreachable policy service as "last known policy" for up to 24 h.

#### Scenario: Org daily limit hit
- **WHEN** the org limit is $20/day and the user's local usage today reaches $20
- **THEN** new provider turns are refused with `organization daily spend limit reached`

### Requirement: Usage commands
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), `/usage` (the current Session tree's breakdown, configured Budgets, provider-reported limits and contributing factors for the last 24 hours or 7 days) and `/stats` (activity, streaks and per-model totals for all time, the last 7 days and the last 30 days). In the TUI these SHALL open the matching tab of the Settings panel. It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--by origin] [--project <id>] [--plugins] [--format json]`.

`--by origin` SHALL report the usage origins defined by `cross-session-messaging` → Usage attribution.

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

#### Scenario: Cost opens the Usage tab
- **WHEN** the user types `/cost` in the TUI
- **THEN** the Settings panel opens on the Usage tab at the Session section

### Requirement: Context window meter
(P1) The system SHALL expose, per session, the current context usage (tokens in the last request versus the model context window and the auto-compaction threshold) via the session status API and the status line data. It SHALL update after each Turn. `GET /api/v1/sessions/:id` and each `session.usage` live event SHALL carry `context: { used_tokens, window, compact_at, utilization, measured, compacted }`, where `used_tokens` is the last finished step's prompt tokens (input + cache read + cache write), `window` the context window of the Session's current model (so switching model re-resolves it), `compact_at` the prompt size above which auto-compaction runs (the window minus the larger of the output allowance and the compaction buffer), and `utilization` their ratio. An unknown window SHALL make `window`, `compact_at` and `utilization` null rather than 0, disabled auto-compaction SHALL make `compact_at` null, and `measured` SHALL be false before the first step and after a compaction until the next step, when `used_tokens` is 0 and only the window and compaction point are known (`compacted` is then true after a compaction). The Session route SHALL report `context: null` only when the Session has no model.

#### Scenario: Status line shows context
- **WHEN** a custom status line script runs
- **THEN** it receives JSON including `context.used_tokens`, `context.window` and `context.compact_at`

#### Scenario: Meter after a step
- **WHEN** a step's request used 100 prompt tokens of a 200,000-token window with a 1,000-token output allowance and buffer
- **THEN** the Session route reports `context` as `{ used_tokens: 100, window: 200000, compact_at: 199000, utilization: 0.0005 }`

### Requirement: Status line data contract
(P1) The system SHALL pass a JSON document on stdin to the configured status line command, at most every 300 ms. The document SHALL include session id, model, mode, agent, cost, budget remaining, context meter, git branch, active goal summary, running workflows and loops counts, and remote-control state. The command's first stdout line SHALL be rendered.

#### Scenario: Goal in status line
- **WHEN** a goal is active
- **THEN** the status line input includes `goal.text` and `goal.iterations`

### Requirement: OpenTelemetry export
(P1) When OpenTelemetry export is enabled (OpenTelemetry configuration), the server SHALL export OTLP over `http/protobuf`, `http/json` or `grpc`, as `telemetry.otel.protocol` and the `OTEL_*` variables select, to the effective endpoint with the effective headers. Only the background server process (`cyber serve`) SHALL export. Clients of it SHALL NOT export, so each served Session is exported once. In-process runs (`cyber exec --embedded` and `--ephemeral`, `cyber --embedded`, `cyber serve --stdio`, `cyber eval` and skill commands) SHALL NOT export their Sessions, even with export enabled. The export SHALL be produced by one exporter that observes the runtime's event stream. It SHALL contain:

- **Metrics** (counters):
  - `cyber.token.usage` by token class (`input`, `output`, `reasoning`, `cache_read`, `cache_write`) and model,
  - `cyber.cost.usage` in USD, where unpriced steps add nothing,

  both over every model call the usage ledger bills (steps, auxiliary calls, generated titles, compactions and auto-mode reviews), so they agree with the durable usage ledger,
  - `cyber.turn.count` by outcome,
  - `cyber.tool.calls` by tool and outcome,
  - `cyber.tool.errors` by tool,
  - `cyber.permission.decisions` by decision (`allow`, `deny`) and source (`user`, `auto`); a request the runtime withdraws (a stop, an abandoned call) is not a decision,
  - `cyber.session.count` by kind (`root`, `subagent`, `workflow_agent`),
  - `cyber.active_time` in seconds of Turn wall time.

  Session, call and run IDs SHALL NOT be metric attributes.
- **Traces:**
  - A `cyber.session` span per activity period of a Session (from its first Turn, or a hook that runs before it, to `Idle`).
  - `cyber.turn` spans under it.
  - `cyber.tool` spans under the Turn that called the tool.
  - `cyber.hook` spans under the tool call, Turn or Session they ran for. A hook that runs while the Session has no activity period (`UserPromptSubmit` at admission, `SessionStart`) opens its session span, which the following Turn joins; when no Turn follows, the span ends with the hook.
  - A `cyber.workflow.run` span as the root of its own trace, with `cyber.workflow.agent` spans under it.
  - A subagent's session span under the parent's Turn span, and a workflow agent's child Session under its agent span.
  - Spans SHALL carry `cyber.session.id` and the IDs of their tool call, run or agent. Permission decisions and sandbox denials and escalations SHALL be span events on their tool span.
- **Logs:** every structured log record (Structured logs) that passes the level filter.

Resource attributes SHALL include:

- `service.name=cyber` (`OTEL_SERVICE_NAME` overrides it),
- `service.version`,
- `os.type`,
- `cyber.account.sub` and `cyber.account.org_id` when a Cyber Account is signed in at server start.

`OTEL_RESOURCE_ATTRIBUTES` SHALL add attributes but SHALL NOT replace these. A span that is open when the server stops SHALL be ended with `cyber.interrupted=true`, and so SHALL the span of a tool call that settles as interrupted. After a restart, work that started before it SHALL NOT get a span: spans SHALL NOT be rebuilt from stored events. Its metrics SHALL still be recorded when it settles, including the work the restart recovery settles or resumes before the server starts listening.

#### Scenario: Traces for a workflow
- **WHEN** OTel is enabled and a workflow runs
- **THEN** a trace with a root span for the run and child spans for each agent and tool call is exported

#### Scenario: Subagent under its Turn
- **WHEN** OTel is enabled and a Turn calls the `agent` tool, whose child Session runs one tool call
- **THEN** the child's `cyber.session` span is a child of the parent's `cyber.turn` span, and the child's tool span is in the same trace

#### Scenario: Metrics for a Turn
- **WHEN** OTel is enabled and a Turn with 1,200 input and 300 output tokens at a priced model calls `read` once and the call fails
- **THEN** the exported metrics include `cyber.token.usage` of 1,200 (`type=input`) and 300 (`type=output`), a positive `cyber.cost.usage`, `cyber.turn.count` 1, `cyber.tool.calls` 1 and `cyber.tool.errors` 1 for `read`

#### Scenario: Restart does not resurrect spans
- **WHEN** the server stops while a tool call runs and the Session continues on a restarted server
- **THEN** the first server exported the tool span with `cyber.interrupted=true`, the second exports no tool span for the call, and `cyber.tool.calls` counts the call once

### Requirement: Prompt content privacy in telemetry
(P1) OpenTelemetry export SHALL NOT include any of the following unless `telemetry.log_prompts` is `true` in global configuration:

- prompt text or model output,
- tool inputs or outputs,
- Session titles or file paths,
- sandbox targets,
- log message text and non-identifier log fields.

Hook stdin and stdout SHALL never be exported. When `log_prompts` is enabled, every exported value SHALL first pass the secret redaction rules (Secret redaction): key-based redaction for structured values, the known credential patterns for text, and the configured header values matched exactly. Each match SHALL be replaced by `***`. Values SHALL be capped at 16 KiB. Header values SHALL never appear in an exported attribute or log record, whatever `log_prompts` is.

#### Scenario: Default omits content
- **WHEN** OTel is enabled with defaults
- **THEN** tool spans carry tool name, duration and outcome but no input or output attributes

#### Scenario: Prompt logging redacts secrets
- **WHEN** `telemetry.log_prompts` is `true` and a prompt contains `my key is sk-abc123def456ghi789`
- **THEN** the Turn span's `cyber.prompt` attribute contains `my key is ***` and not the key

#### Scenario: Project cannot enable prompt logging
- **WHEN** a trusted project's `cyber.jsonc` sets `telemetry.log_prompts: true` and global configuration does not
- **THEN** no content attribute is exported and `cyber telemetry show` lists the project value as ignored

### Requirement: Structured logs
(P0) The system SHALL write structured JSON-lines logs to `<data>/log/cyber-<YYYY-MM-DD>.log`, one file per day, keeping 14 days of files (the contract shared with `cli-commands` → Logging destination), at level `info` by default (`--log-level`, `CYBER_LOG_LEVEL`). `--print-logs` SHALL additionally mirror logs to stderr. Each line SHALL include timestamp, level, component, and when applicable session id and request id.

#### Scenario: Rotation
- **WHEN** a log file is older than 14 days at startup
- **THEN** it is deleted

### Requirement: Debug traces
(P1) When `CYBER_TRACE=1` or `--trace` is set, the system SHALL write a JSONL trace per process to `~/.local/share/cyber/log/trace/<timestamp>-<pid>.jsonl`, recording provider requests and responses (with secrets redacted), tool calls, permission decisions, hook executions and event-bus messages, and SHALL update `latest.json` to point to it.

#### Scenario: Trace for a bug report
- **WHEN** the user reproduces an issue with `cyber --trace`
- **THEN** a redacted JSONL trace file is written and its path printed on exit

### Requirement: Secret redaction
(P0) The system SHALL redact secrets in logs, traces, telemetry, `cyber doctor` output and exported diagnostics. Secrets SHALL include values of keys matching `api[_-]?key|secret|password|token|authorization|cookie|credential|private[_-]?key`, header values, URL userinfo and secret-named query params, values loaded from the keyring, and strings matching known key formats (`sk-`, `ghp_`, `AKIA`, JWTs). Each SHALL be replaced with `***`.

#### Scenario: Bearer header redacted
- **WHEN** a provider request with `Authorization: Bearer sk-abc` is traced
- **THEN** the trace shows `Authorization: ***`

### Requirement: Product telemetry opt-in
(P0) Anonymous product telemetry (feature usage counts, crash reports without code or prompts) SHALL be disabled by default. It SHALL be enabled only by `telemetry.product: true` or an explicit opt-in prompt answer, and SHALL be disabled whenever `DO_NOT_TRACK=1` or `CYBER_TELEMETRY=0`. Payloads SHALL be documented and viewable with `cyber telemetry show`.

#### Scenario: DO_NOT_TRACK respected
- **WHEN** `DO_NOT_TRACK=1` is set and config enables product telemetry
- **THEN** no product telemetry is sent

### Requirement: Doctor health checks
(P1) The system SHALL provide `cyber doctor` and `/doctor`. They SHALL check config validity (with paths of errors), provider reachability and credential validity for configured providers, sandbox availability (Seatbelt / bubblewrap / Landlock / Windows), git and ripgrep availability, LSP and formatter binaries, MCP server statuses, plugin host health, database integrity (`PRAGMA quick_check`), disk space in data dirs, Cyber Account token validity, and relay connectivity when remote control is enabled. Each check SHALL print `ok`, `warn` or `fail` with a remediation hint, and the command SHALL exit non-zero on any `fail`.

#### Scenario: Missing bubblewrap
- **WHEN** `cyber doctor` runs on Linux without `bwrap`
- **THEN** the sandbox check prints `fail` with the install hint and the exit code is 1

### Requirement: Performance diagnostics
(P1) The system SHALL provide `cyber debug perf`, reporting startup phase timings, per-Location service build times, memory RSS, open file descriptors, SQLite size and WAL size. With `CYBER_AUTO_HEAP_PROFILE=1`, it SHALL write a heap profile to the log directory whenever RSS crosses 2 GiB, at most once per 10 minutes.

#### Scenario: Startup timings
- **WHEN** the user runs `cyber debug perf`
- **THEN** a table of startup phases with durations in milliseconds is printed

### Requirement: Usage data retention
(P1) Usage records SHALL be kept for `telemetry.usage_retention_days` (default 365) and pruned daily. Pruning SHALL keep daily aggregates so that `cyber stats` totals remain available for older periods.

#### Scenario: Old records aggregated
- **WHEN** usage records are older than the retention window
- **THEN** per-message records are deleted and daily aggregates remain

### Requirement: Diagnostics bundle
(P1) The system SHALL provide `cyber debug bundle [--session <id>]`, writing a zip with redacted config, versions, doctor output, recent logs (last 2,000 lines) and optionally the redacted session trace, for attaching to bug reports. It SHALL print the bundle path and size.

#### Scenario: Bundle excludes secrets
- **WHEN** the user creates a diagnostics bundle
- **THEN** the zip contains redacted config only and no keyring values

### Requirement: Budget reservations and unknown usage
(P0) Budgets SHALL default to `enforcement: soft`: they block new calls at the recorded limit and disclose possible in-flight overshoot. Optional `enforcement: reserved` SHALL atomically reserve conservative input and capped output cost before dispatch across every applicable scope, including descendants, title, evaluator, advisor and compaction calls. Reservation failure SHALL block dispatch. Reserved USD budgets SHALL reject unknown pricing with `BudgetUnpricedError`; a request without an accepted adapter bound SHALL fail with `BudgetBoundUnavailableError`, including token-only requests. Unknown pricing SHALL be reported first when both conditions apply. Actual usage SHALL reconcile reservations once by request ID; missing usage after interruption SHALL retain the reservation as uncertain until reconciled or explicitly released. Reservations bound scheduling estimates, not a provider billing guarantee. UI and reports SHALL disclose estimated charges, uncertainty and any overrun.

#### Scenario: Parallel agents compete for remaining funds
- **WHEN** two calls each require a $0.75 reservation and $1.00 remains in their shared budget
- **THEN** only one call reserves funds and starts; the other waits or returns budget exhausted

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

### Requirement: OpenTelemetry configuration
(P1) Export settings SHALL be read from global configuration and the environment only. A project layer's `telemetry.otel` and `telemetry.log_prompts` SHALL be ignored. The keys SHALL be:

- `telemetry.otel.enabled` (default `false`),
- `telemetry.otel.endpoint` (a base URL),
- `telemetry.otel.protocol` (`http/protobuf` default, `http/json`, `grpc`),
- `telemetry.otel.headers` (an object of strings whose values are secrets per `configuration` → Secrets handling),
- `telemetry.log_prompts` (default `false`).

The effective settings SHALL be resolved in this order:

1. **Off switches:** `CYBER_DISABLE_TELEMETRY` (truthy) or `OTEL_SDK_DISABLED=true` SHALL turn export off.
2. **Managed policy:** a managed policy document that is present but malformed SHALL turn export off. A managed telemetry setting, when org policy defines one, SHALL win over every user setting.
3. **Enablement:** export SHALL be on only when `telemetry.otel.enabled` is `true`. No `OTEL_*` variable SHALL enable it.
4. **Values:** a per-signal variable (`OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_{ENDPOINT,PROTOCOL,HEADERS,TIMEOUT}`) SHALL beat the generic `OTEL_EXPORTER_OTLP_*` variable, which SHALL beat `telemetry.otel.*`, which SHALL beat the default.
   - A headers variable SHALL replace the configured headers as a whole.
   - For HTTP, a generic or configured endpoint SHALL get `/v1/traces`, `/v1/metrics` or `/v1/logs` appended, and a per-signal endpoint SHALL be used as given.
   - The default endpoint SHALL be `http://localhost:4318`, or `http://localhost:4317` for `grpc`.
5. **Per-signal switch:** `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none` SHALL turn that signal off.

Any of the following SHALL keep export off and report the reason in `cyber telemetry show` and in one log line, without failing server start:

- an invalid endpoint or protocol,
- a non-string header,
- an unsupported protocol in this build,
- headers with a non-loopback plain `http://` endpoint.

Settings SHALL be resolved at server start.

#### Scenario: Endpoint variable alone does not enable export
- **WHEN** `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318` is set and configuration does not set `telemetry.otel.enabled`
- **THEN** nothing is exported and `cyber telemetry show` reports export off with the reason `telemetry.otel.enabled is not true`

#### Scenario: Environment overrides configuration
- **WHEN** global configuration enables export with `endpoint: "https://otel.acme.com"` and `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:9999/v1/traces` is set
- **THEN** traces go to `http://127.0.0.1:9999/v1/traces` with source `env (OTEL_EXPORTER_OTLP_TRACES_ENDPOINT)`, and metrics go to `https://otel.acme.com/v1/metrics` with source `config`

#### Scenario: SDK disabled
- **WHEN** export is enabled in configuration and `OTEL_SDK_DISABLED=true` is set
- **THEN** nothing is exported and `cyber telemetry show` names `OTEL_SDK_DISABLED` as the reason

#### Scenario: Malformed managed policy keeps export off
- **WHEN** export is enabled in configuration and the OS-managed policy document is malformed
- **THEN** nothing is exported and `cyber telemetry show` reports the managed policy error as the reason

#### Scenario: Headers not sent in clear text
- **WHEN** configuration enables export to `http://collector.lan:4318` with an `authorization` header
- **THEN** export stays off and the reason says headers need an `https` or loopback endpoint

### Requirement: OpenTelemetry delivery
(P1) Export SHALL never block, slow down or fail a Turn, a tool call or any other runtime work.

- **Batching:** signals SHALL be batched through bounded queues.
- **Counters:** events the exporter could not keep up with SHALL be counted as `dropped`. Items in batches the endpoint rejected or could not receive SHALL be counted as `failed`, per signal.
- **Report once:** an unreachable or failing endpoint SHALL be reported once when export starts failing (a `warn` log line and `last_error` in `cyber telemetry show`) and once when it recovers, not once per batch or event.
- **Shutdown:** the server SHALL end open spans, flush every signal, and give up after 3 seconds in total, then log the final counters. What is not exported by then is dropped.

#### Scenario: Unreachable collector
- **WHEN** export is enabled to an endpoint that refuses connections and a Session runs three Turns
- **THEN** all three Turns complete normally, the log has one `telemetry export failing` line, and `cyber telemetry show` reports a non-zero `failed` count and the last error

#### Scenario: Flush at shutdown
- **WHEN** export is enabled with a 60 s metric interval and the server stops 1 s after a Turn ends
- **THEN** the collector receives that Turn's spans and metrics before the server exits, and the server exits within 3 s of the stop

### Requirement: Durable provider reservation lifecycle
(P2) For reserved enforcement, the server SHALL atomically persist a request-identified reservation in every applicable budget scope before provider dispatch, including auxiliary calls and descendants. Admission SHALL compare recorded usage plus outstanding reservations against each limit using conservative bounded estimates. USD requests without pricing SHALL fail with `BudgetUnpricedError` before unsupported-bound errors; requests without an accepted adapter bound SHALL fail with `BudgetBoundUnavailableError`, including token-only requests, without dispatch or soft fallback. Settlement SHALL reconcile actual usage and unused allocation once by request identity; interruption after possible dispatch without authoritative usage SHALL retain an uncertain allocation across restart. Only authoritative reconciliation or an explicit audited owner release SHALL clear that uncertainty. Existing soft enforcement SHALL remain unchanged.

#### Scenario: Concurrent requests compete for the final allocation
- **WHEN** two provider requests individually fit a reserved scope but their combined estimates exceed its remaining allowance
- **THEN** the atomic ledger admits only a request whose allocation fits and the refused request makes no provider call

#### Scenario: Crash after possible dispatch
- **WHEN** the server restarts after a reserved request may have reached the provider but no final usage was recorded
- **THEN** its allocation remains uncertain and unavailable for new dispatch until explicitly reconciled or released

#### Scenario: Duplicate settlement
- **WHEN** a request's usage callback or durable recovery is processed twice
- **THEN** recorded spend and released allocation change only once

#### Scenario: Auxiliary call cannot evade a run budget
- **WHEN** a workflow descendant's title, evaluator, advisor or compaction request would exceed an applicable reserved limit
- **THEN** admission is refused before network dispatch and the report identifies that budget and purpose

#### Scenario: Midnight retains unresolved capacity
- **WHEN** a reserved request crosses the server local-day boundary
- **THEN** its remaining allocation SHALL continue to constrain the stable daily scope
- **AND** newly committed cumulative-receipt deltas SHALL charge their receipt day exactly once, including later audited reconciliation.

#### Scenario: Canonical turn retries
- **WHEN** a provider opening retry shares a Session and canonical message identity
- **THEN** it SHALL share one turn claim while retaining independent token/cost allocations
- **AND** committed successful receipt evidence SHALL count the turn before its canonical transcript event commits.
