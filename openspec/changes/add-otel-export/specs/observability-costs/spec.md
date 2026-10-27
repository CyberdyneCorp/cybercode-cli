## MODIFIED Requirements

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

## ADDED Requirements

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
