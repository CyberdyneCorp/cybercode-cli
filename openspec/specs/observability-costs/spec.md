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
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), and `/usage` (today, last 7 and 30 days for this machine). It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--project <id>] [--plugins] [--format json]`.

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

### Requirement: Context window meter
(P1) The system SHALL expose, per session, the current context usage (tokens in the last request versus the model context window and the auto-compaction threshold) via the session status API and the status line data. It SHALL update after each Turn.

#### Scenario: Status line shows context
- **WHEN** a custom status line script runs
- **THEN** it receives JSON including `context.used_tokens`, `context.window` and `context.compact_at`

### Requirement: Status line data contract
(P1) The system SHALL pass a JSON document on stdin to the configured status line command, at most every 300 ms. The document SHALL include session id, model, mode, agent, cost, budget remaining, context meter, git branch, active goal summary, running workflows and loops counts, and remote-control state. The command's first stdout line SHALL be rendered.

#### Scenario: Goal in status line
- **WHEN** a goal is active
- **THEN** the status line input includes `goal.text` and `goal.iterations`

### Requirement: OpenTelemetry export
(P1) When `telemetry.otel.enabled` is true, the system SHALL export OTLP (gRPC or HTTP per `telemetry.otel.protocol`) to `telemetry.otel.endpoint` with configured headers. The export SHALL contain metrics (tokens, cost, turns, tool calls, tool errors, permission decisions, session count, active time), traces (spans per session, turn, tool call, subagent, workflow agent, hook) and logs. Resource attributes SHALL include `service.name=cyber`, version, os, and the account `sub` and org id when signed in. Standard `OTEL_*` environment variables SHALL be honored.

#### Scenario: Traces for a workflow
- **WHEN** OTel is enabled and a workflow runs
- **THEN** a trace with a root span for the run and child spans for each agent and tool call is exported

### Requirement: Prompt content privacy in telemetry
(P1) OpenTelemetry export SHALL NOT include prompt text, model output, tool inputs or tool outputs unless `telemetry.log_prompts` is true. Even when enabled, values matching the secret redaction rules SHALL be replaced by `***`.

#### Scenario: Default omits content
- **WHEN** OTel is enabled with defaults
- **THEN** tool spans carry tool name, duration and outcome but no input or output attributes

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
(P0) Budgets SHALL default to `enforcement: soft`: they block new calls at the recorded limit and disclose possible in-flight overshoot. Optional `enforcement: reserved` SHALL atomically reserve conservative input and capped output cost before dispatch across every applicable scope, including descendants, title, evaluator, advisor and compaction calls. Reservation failure SHALL block dispatch. Reserved USD budgets SHALL reject unknown pricing or an unbounded provider charge with `BudgetUnpricedError` until an explicit price bound or token budget is supplied. Actual usage SHALL reconcile reservations once by request ID; missing usage after interruption SHALL retain the reservation as uncertain until reconciled or explicitly released. Reservations bound scheduling estimates, not a provider billing guarantee. UI and reports SHALL disclose estimated charges, uncertainty and any overrun.

#### Scenario: Parallel agents compete for remaining funds
- **WHEN** two calls each require a $0.75 reservation and $1.00 remains in their shared budget
- **THEN** only one call reserves funds and starts; the other waits or returns budget exhausted
