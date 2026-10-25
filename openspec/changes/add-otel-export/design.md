## Context

- **No tracing stack today.** Logging is the custom JSON-lines writer `cyber_core::log`: one global `Logger` with `init`, `log`, `error|warn|info|debug`, daily files and 14 days kept.
- **Durable and live events share one bus.** Every committed Session event and every workflow-run event is published as `LiveEvent::Durable { session_id (the aggregate id), seq, kind, data }` on the runtime's broadcast `Bus` (`crates/cyber-server/src/runtime/bus.rs`, capacity 4096; `Runtime::subscribe`). Live-only fragments (`Usage`, `Idle`, `ToolProgress`, `Retry`, `Error`, …) travel beside them.
- **What the bus carries for this change:**
  - Turns: `session.prompt.admitted.1`, `session.prompt.promoted.1` and `session.step.started|ended|failed.1` (usage and cost are on `step.ended`), then `Idle` when a Drain ends.
  - Text: `session.text.ended.1`.
  - Tools: `session.tool.called|dispatched|settled.1`.
  - Permissions: `permission.asked|replied.1` and `permission.auto_decided.1`.
  - Sandbox: `sandbox.denied.1` and `sandbox.escalated.1`.
  - Sessions: `session.created.1`, which carries `parent_id`, `subagent_name` and `workflow_run_id`.
  - Workflow runs: `workflow.run.*` and `workflow.agent.started|settled.1` (aggregate `run_…`).
- **Hook receipts are not on the bus.** `hook.started.1` and `hook.executed.1` are appended to the store by `runtime/hooks.rs` (aggregate `hook_…`) and are never published.
- **Configuration:**
  - `telemetry` is already an accepted top-level key (`config/validate.rs`). It is a sensitive key in project layers (`config/gate.rs`): project definitions stay inactive until the checkout is trusted.
  - Only `telemetry.usage_retention_days` and `telemetry.log_hook_io` are read today.
  - `cyber telemetry` is listed in `crates/cyber-cli/src/tree.rs` and answers "not available".
- **Managed policy** (`config/managed.rs`): it accepts only `$schema`, `hooks`, `mcp` and `policy`, and `policy.*` only `autoupdate|features|hooks|messaging|mcp|modes|plugins|sandbox`. A document with `telemetry` or `policy.telemetry` is malformed and fails closed. There is no managed telemetry layer to respect yet.
- **Redaction exists in two forms:**
  - `cyber_core::config::redact_secrets(&Value)`: key-based, `***`, every value under `headers`.
  - `cyber_core::memory::secrets::redact_credentials(&str)`: credential patterns and secret assignments, with the marker `[REDACTED]`.
- **Account:** `cyber_account::Account { sub, org: Option<Organization { id, .. }> }`. The server reaches it through `cyber-app`'s `Accounts`.
- **Workspace and targets:** the workspace uses `reqwest 0.13.5`, `tokio 1.53`, `hyper 1`, `rustls 0.23` on `aws-lc-rs`, and `axum 0.8`. CI builds all 8 release targets on every PR (`ci.yml` Build matrix: macOS ×2, Linux gnu ×2, musl ×2, Windows x64 and arm64).

## Goals / Non-goals

- **Goals:** the two P1 requirements and `cyber telemetry show`, through one exporter that observes the runtime instead of instrumentation in every crate. Privacy by default. No effect on Turns.
- **Non-goals** (see the proposal for the reasons):
  - org policy for telemetry beyond failing closed on a malformed managed document,
  - exemplars and histogram buckets,
  - a bundled collector or dashboards,
  - TLS client certificates,
  - product telemetry.

## Decisions

### One crate, one subscriber

- **`crates/cyber-telemetry` (new)** depends on `cyber-core` (settings, redaction, log sink), `cyber-server` (the `LiveEvent` type and a read-only Session lookup) and the OpenTelemetry crates. It has four modules:
  - `settings`: the resolved settings with a source for every value.
  - `mapper`: a pure event-to-signal state machine over SDK tracer, meter and logger handles.
  - `pipeline`: the providers, the exporters and the counting wrapper.
  - `status`: the counters behind `cyber telemetry show`.
- **`cyber-app` wires it** in `run_server_until`, next to the retention sweeper:
  - `Telemetry::start(settings, runtime.subscribe(), lookup, resource)` returns a handle.
  - The handle is put in `http::AppState` for the status route.
  - The handle is shut down after `stop` is notified.
- **Only the served App exports.** Clients of the server (TUI, `exec`, `api`) never export; their Sessions run in the server, so each is exported exactly once. In-process Apps (`exec --embedded`/`--ephemeral`, `tui --embedded`, `serve --stdio`, `eval`, skill commands) never export either: they never call `run_server_until`, and an ephemeral run would end before a batch is sent. `App::build_served` (what `cyber serve` calls) takes the early subscription; `App::build` takes none.
- **No crate emits spans or metrics itself.** The mapper is the only producer. The one new runtime publication is the hook receipt below.

### Settings and precedence

- **Global configuration only.** The settings are read by `config::load_telemetry`, which returns the `telemetry` section of the global layers and the environment layers (`CYBER_CONFIG` and `CYBER_CONFIG_CONTENT`, which the user sets, never a checkout) with its `sources`, plus the managed-policy state and the ignored project pointers. There are no telemetry defaults in the defaults layer: an unset value is sourced `default`. It works like `load_owner_server`: an unrelated configuration error cannot disable or enable export.
- **Validation lives outside `config::load`.** The typed section (`TelemetrySettings::from_config`) is checked only by `load_telemetry`, so an invalid `telemetry.otel` value keeps export off and never makes a Location's configuration invalid or a Turn fail.
- **Project layers are ignored.** `telemetry.otel` and `telemetry.log_prompts` in a project layer are not read, even in a trusted checkout, and `cyber telemetry show` lists them as ignored. A repository must not be able to send a user's Sessions to its own collector or turn on content capture.
- **Precedence, highest first:**
  1. **Kill switches:** `CYBER_DISABLE_TELEMETRY` (truthy per `configuration` → Environment variables) or `OTEL_SDK_DISABLED=true` turns export off.
  2. **Managed policy:** a managed policy document that is present but malformed turns export off (fail closed). There is no managed telemetry key in this change. When org policy adds one (P3), it slots in here, above every user setting.
  3. **Enablement:** export is on only when `telemetry.otel.enabled` is `true` in global configuration. An `OTEL_*` endpoint, protocol or header variable alone never enables export. Many machines set `OTEL_EXPORTER_OTLP_ENDPOINT` for other programs, and picking it up would send Cyber Sessions somewhere the user never chose.
  4. **Value precedence (once enabled):** for each value, a per-signal variable beats the generic variable, which beats `telemetry.otel.*`, which beats the default. This matches the CLI rule "environment variables over config".
     - `OTEL_EXPORTER_OTLP_{TRACES,METRICS,LOGS}_ENDPOINT` beats `OTEL_EXPORTER_OTLP_ENDPOINT`, which beats `telemetry.otel.endpoint`, which beats the default.
     - The same chain applies to `_PROTOCOL`, `_HEADERS` and `_TIMEOUT`.
     - A headers variable replaces the configured header map as a whole. It is not merged key by key, so the effective set always comes from one source and `show` can name it.
  5. **Per-signal switch:** `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none` turns one signal off. Any other value except `otlp` is reported as unsupported, and that signal stays off.
- **Defaults:**
  - Protocol `http/protobuf`.
  - Endpoint `http://localhost:4318`, or `http://localhost:4317` for `grpc`.
  - Timeout 10 s.
  - For HTTP, the generic endpoint is a base URL and `/v1/traces`, `/v1/metrics` or `/v1/logs` is appended. A per-signal endpoint is used as is (OTLP exporter specification).
- **Validation fails closed.** Each of these leaves export off, with the reason in `show` and one `warn` log line. The server still starts:
  - an endpoint that is not an absolute `http`/`https` URL,
  - an unknown protocol,
  - a non-string header value,
  - a `grpc` protocol in a build without the `otlp-grpc` feature,
  - headers with a plain `http://` endpoint whose host is not loopback. Header values are credentials and are never sent in clear text off the machine.
- **When settings take effect.** Settings are resolved once at server start, like `server.*`. A change takes effect on `cyber service restart`, and `show` says so when the configuration on disk differs from the running settings.
- **Resource:**
  - `service.name`: `cyber`, overridden by `OTEL_SERVICE_NAME`.
  - `service.version`: the binary version, and `cyber.build` its build identity.
  - `os.type` (`darwin`, `linux`, `windows`).
  - `cyber.account.sub` and `cyber.account.org_id` when an account is signed in at server start.
  - `OTEL_RESOURCE_ATTRIBUTES` adds attributes but cannot replace `service.version`, `os.type` or the `cyber.*` keys. Signing in or out applies at the next server start.

### Event-to-span mapping

Span start and end times are the times the mapper observes the events. Bus delivery is immediate, and `Durable` carries no timestamp. Hook spans use the receipt's `started_ms` and `duration_ms`.

| Span | Starts on | Ends on | Parent |
|---|---|---|---|
| `cyber.workflow.run` | `workflow.run.started.1` | `workflow.run.completed\|failed\|stopped.1` | none: the root of its own trace. It links to the owner Session's open Turn span when there is one. |
| `cyber.workflow.agent` | `workflow.agent.started.1` | `workflow.agent.settled.1` | the run span |
| `cyber.session` | the first Turn of a Session, or a hook receipt admitted, while no span for it is open | `Idle` for that Session | a workflow child Session: its agent span (matched by `subagent_name` = agent id, or the agent's `session_id`). A subagent: the parent Session's open Turn span, else the parent's session span. A root Session: none (a new trace). |
| `cyber.turn` | `session.prompt.promoted.1`. A promotion while the open Turn has no step yet joins it, because one promotion commit can promote several inputs. A `session.step.started.1` with no open Turn starts one: a child Session's first input is part of its creation and is never promoted. | the next Turn's promotion or `Idle`. A failed last step (`session.step.failed.1`) makes the Turn `failed` with an `Error` status; one of kind `interrupted` (a stop) makes it `interrupted` | the session span |
| `cyber.tool` | `session.tool.called.1` | `session.tool.settled.1` | the open Turn span |
| `cyber.hook` | the hook receipt's start | the hook receipt's settlement | the tool span with the receipt's `call_id`, else the open Turn span, else the session span, chosen when the receipt is admitted. A receipt admitted while the Session has no open span (`UserPromptSubmit` runs at admission, `SessionStart` before the first promotion, configuration hooks at any time) starts the session span at the receipt's `started_ms`; a following Turn joins it, and when the receipt leads to no Turn (any event but `SessionStart` and an allowed `UserPromptSubmit`) the span ends when the receipt settles. The span is recorded at settlement with the receipt's `started_ms` and `duration_ms`. |

- **Session span lifetime.** A session span covers one activity period (a Drain), not the Session's whole lifetime. A Session can live for weeks, and a span that never ends is never exported. Every span carries `cyber.session.id`, so all periods of one Session can be queried together.
- **Sessions first seen after a restart.** When the first event of a Session has no `session.created.1` behind it in this process (it was created before a restart), the mapper reads its `SessionInfo` once through a read-only lookup on `Runtime` (`Runtime::session_info`, without the history) and caches `parent_id`, `subagent_name`, `workflow_run_id`, `agent` and `model`. The cache is bounded (LRU, 4096 entries). A Session the lookup cannot read is treated as a root Session.
- **Attributes, always:**
  - `cyber.session.id` and `cyber.session.kind` (`root`, `subagent`, `workflow_agent`).
  - On Turns: `cyber.agent`, `cyber.mode` and `cyber.model`.
  - On tool spans: `cyber.tool.name`, `cyber.call.id`, `cyber.tool.attempt` and `cyber.tool.outcome` (the `CallStatus`). The span status is `Error` when the call failed or was denied.
  - On run spans: `cyber.workflow.run_id`, the workflow name and status.
  - On agent spans: `cyber.workflow.agent_id`, phase, model, status, cost and tokens.
  - On hook spans: `cyber.hook.id`, `cyber.hook.event`, `cyber.hook.scope`, `cyber.hook.outcome` and the decision kind.
  - On Turns, from the steps they contain: the tokens by class (`cyber.tokens.<class>`), the cost (`cyber.cost.usd`, absent when unpriced) and the finish reason (`cyber.finish_reason`). `cyber.agent`, `cyber.mode` and `cyber.model` (the step's model id; its provider is `cyber.provider` on metrics) come from the Turn's latest step and are set once, when the Turn ends.
  - `cyber.tool.attempt` is the call's last `session.tool.dispatched.1` attempt (1 when it never dispatched), set when the call settles.
- **Span events:**
  - A tool span gets `permission.decision` (`decision`, `source`: `user`, `auto` or `rule`) from `permission.replied.1` and `permission.auto_decided.1`.
  - A tool span gets `sandbox.denied` and `sandbox.escalated` with `operation`. That meets `sandbox` → Sandbox events "exported via telemetry".
  - A Turn span gets `retry` from `Retry`, with the attempt number and the delay.
- **Content attributes,** only with `telemetry.log_prompts` and always redacted (below):
  - Turns: `cyber.prompt` (the promoted input's text parts, from its `admitted` event; the mapper keeps admitted text only while `log_prompts` is on) and `cyber.response` (from `session.text.ended.1`).
  - Tool spans: `cyber.tool.input` (arguments) and `cyber.tool.output`.
  - Sandbox events: `target`.
  - `cyber.session.title` and `cyber.directory`.
  - Each value is capped at 16 KiB and ends with `…[truncated]` when cut.
- **Hook receipts on the bus.**
  - `runtime/hooks.rs` publishes `LiveEvent::HookReceipt { session_id, receipt }` when a receipt is admitted and when it settles (`HookExecution` gets a `Bus` clone). The receipt is a summary struct with id, hook id, event, scope, `call_id`, tool name, `started_ms`, `duration_ms`, outcome and decision kind. It has no IO field, so hook IO cannot reach telemetry even with `log_hook_io`.
  - The variant is internal. `http/events.rs` drops it from every SSE, WebSocket and MCP stream, and it is skipped in the OpenAPI schema, so no client API changes.
  - Skipped receipts (`record_skipped_hook`) publish a settled receipt with zero duration.
- **Account at start.** `run_server_until` reads the signed-in account through the account service only when export is on, waiting at most 2 s; it never refreshes past that.
- **Increment order.** The logs provider and the log bridge arrived with the log sink (task 3.1). The logs signal uses the same protocols as the others (`http/protobuf`, `http/json`, `grpc`) and the SDK batch log processor on the server's Tokio runtime.
- **Shutdown and restart.**
  - At shutdown, every open span ends with `cyber.interrupted=true` and no status, before the providers flush. The runtime's shutdown settles a running tool call as `interrupted` before that (its Drain is cancelled), and a call that settles as `interrupted` ends its span with `cyber.interrupted=true` too. So the call is counted by the server that stopped, and the restarted server has nothing left to settle for it.
  - The exporter's subscription is taken in `App::build` before the restart recovery (`recover_jobs`, `recover_teams`, `recover_loops`, Goal recovery, `recover_workflow_runs` and the resumes of `workflows.resume_after_restart`), when the settings turn export on, so the events that recovery publishes reach the mapper. The broadcast buffer holds 4096 events until the server starts the exporter; more are counted as dropped.
  - After a restart, events that settle work started before it (a `tool.settled` with no open tool span, an `agent.settled` with no agent span) create no span. Their metrics are still recorded, because metrics are counted at settlement.
  - Unfinished spans are never resurrected from the event store. Replaying history to rebuild spans would export spans with fabricated parents and timings.

### Metrics

Every metric is a counter, recorded when work settles, with cumulative temporality. The interval is 60 s; the SDK honors `OTEL_METRIC_EXPORT_INTERVAL`.

| Metric | Unit | Attributes | From |
|---|---|---|---|
| `cyber.session.count` | `{session}` | `cyber.session.kind` | `session.created.1` |
| `cyber.turn.count` | `{turn}` | `outcome` (`completed`, `failed`, `interrupted`), `cyber.agent` | Turn end |
| `cyber.token.usage` | `{token}` | `type` (`input`, `output`, `reasoning`, `cache_read`, `cache_write`), `cyber.model`, `cyber.provider` | `session.step.ended.1`, `usage.recorded.1`, `session.compaction.completed.1`, and `session.title.generated.1` and `permission.auto_decided.1` when they carry `usage`: the calls `usage_ledger` bills. The model is the event's (`model`, or the evaluator), else the Session's, split as the ledger splits it. |
| `cyber.cost.usage` | `USD` | `cyber.model` | the same events. Unpriced steps add nothing and are never counted as `0`. |
| `cyber.tool.calls` | `{call}` | `cyber.tool.name`, `outcome` | `session.tool.settled.1` |
| `cyber.tool.errors` | `{call}` | `cyber.tool.name` | settled with a failed or denied status |
| `cyber.permission.decisions` | `{decision}` | `decision` (`allow`, `deny`), `source` (`user`, `auto`), `cyber.permission.action` | `permission.replied.1`, `permission.auto_decided.1` |
| `cyber.active_time` | `s` | `cyber.session.kind` | Turn wall time, at Turn end |

- **Tool names after a restart.** `session.tool.settled.1` carries no tool name. A call made before a restart settles with `cyber.tool.name=unknown`.
- **Errors.** `cyber.tool.errors` counts calls that settle with status `error` (a denied call settles so too).
- **Which permission decisions are counted.** Decisions that a rule settles without a request leave no event today and are not counted. The metric counts decisions made by a person or the auto classifier. When the runtime withdraws a pending request (a stop, a subtree stop, a job's end, an abandoned call), its `permission.replied.1` carries `cancelled: true` and is neither counted nor a span event.
- **Cardinality.** Attribute values are bounded sets: models, tools and agents. Session, call and run ids are never metric attributes.

### Logs

- **A record sink in `cyber_core::log`.** `cyber_core::log::set_sink(Option<Sink>)`, with `Sink = Arc<dyn Fn(&LogRecord) + Send + Sync>`, receives each record that passes the level filter after it is written to the file. `LogRecord` holds `ts`, `level`, `component`, `msg` and `fields` (without nulls). The sink keeps the file writer's semantics: no sink means no change, and nothing reaches it before `log::init`.
  - The sink is replaceable rather than set once, because one process can run several servers one after another (tests). The exporter sets it at start and removes it with `clear_sink(&sink)` at shutdown, which only removes that same sink.
  - The sink runs on the caller of `log`, after the file lock is released.
- **The OTLP log bridge.** The bridge turns a record into an OTLP log record:
  - severity from the level,
  - body = `msg`, only with `log_prompts`, through `Content::value` (redacted and capped). Many messages quote file paths and error text (`Could not truncate <path>: <error>`, an internal error's detail), so without `log_prompts` a record has no body and is told apart by its severity, `component` and identifier fields,
  - attributes `component` plus the identifier fields `session_id`, `request_id`, `run_id`, `call_id`, `kind`, `status` and `pid`,
  - with `log_prompts`, the other scalar fields too, redacted (strings through `Content::value`, so also capped). Objects and arrays are never exported, and a field never replaces `ts`, `level`, `component` or `msg`.
  - Identifier strings also pass through the redaction, which leaves ordinary ids unchanged.
  - Logs inside an open span carry no trace context in this change, because log calls do not know their span.
- **No feedback loop.** Emitting is a non-blocking enqueue into the SDK batch processor. The SDK's `internal-logs` feature is disabled, so the OTel SDK never writes to our log, stderr or the sink.

### Privacy and redaction

- **One gate.** `Content::value(&self, text) -> Option<String>` returns `None` unless `log_prompts` is on. Every content attribute goes through it, and no other code path adds content.
- **Redaction with `log_prompts`:**
  - JSON values (tool input) go through `redact_secrets`, then every string leaf through the credential patterns.
  - Text goes through the credential patterns: `secrets::redact_credentials_with(text, "***")`, which is the existing function with a caller-chosen marker; `redact_credentials` keeps `[REDACTED]`.
  - Exact values: the configured header values are replaced wherever they appear.
- **Headers stay secret.** Header values exist only in the exporter's transport configuration:
  - never in an attribute or a log line,
  - in `show` and in the status route as `name=***`,
  - in error messages, which pass through the exact-value redaction before they are stored as `last_error`.
  - The endpoint is shown without userinfo and query.

### Delivery: never block, report once

- **The mapper task** reads the broadcast receiver. `RecvError::Lagged(n)` adds `n` to `dropped` (signal `events`). The mapper never awaits the network. Starting, ending and recording spans and metrics are in-memory SDK calls.
- **Pipelines:** the batch span and log processors use a bounded queue (2048 spans or records, batches of 512, 5 s delay; the `OTEL_BSP_*` and `OTEL_BLRP_*` variables are honored) and a periodic metric reader. They run on the server's Tokio runtime (`opentelemetry_sdk` `rt-tokio`), so `reqwest` and `tonic` exporters share it.
- **Counting wrapper.** Each exporter is wrapped in a `Counted<E>` (`cyber_telemetry::counted`) that records `exported` (items acknowledged), `failed` (items in batches that errored) and `last_error { message, at_ms }` per signal. Items are spans, log records, or for metrics the metrics (instruments) of the export.
  - The failing state is per signal, but the report is shared by the three signals, because one unreachable collector fails them all and the scenario "Unreachable collector" asks for one line: the first signal to start failing while no other is failing logs one `warn` line, `telemetry export failing`, with the signal, the masked endpoint (`display_endpoint`) and the redacted error. Further failures, of any signal, only count.
  - When the last failing signal succeeds again, one `info` line, `telemetry export recovered`, is logged.
  - The error text goes through `Content::redact` (the exact header values, then the credential patterns) before it is logged or stored, whatever `log_prompts` says.
  - `RecvError::Lagged(n)` adds `n` to `dropped_events` in the status; the per-signal `dropped` stays 0 (the SDK queue gap below).
  - Known gap: the SDK's own queue overflow has no public counter in `opentelemetry_sdk 0.33`. It is not in `dropped`. Its queue is sized so that the mapper's bus lag shows up first.
- **Turns are isolated.** Nothing in the Turn path calls the exporter. A slow or unreachable collector only fills SDK queues, which drop. The integration test proves this with a collector that never answers.
- **Shutdown.** After `stop`:
  1. The open spans end.
  2. The log sink is removed, so later log lines go to the file only.
  3. `force_flush` and then `shutdown` run on all three providers, each on its own blocking thread, so a signal stuck on a collector that never answers does not hold up the others.
  4. All of it is bounded by 3 s in total.
  5. Whatever has not been exported by then is dropped; the SDK does not report how much, so it is not counted. The final counters (`exported` and `failed` per signal, `dropped_events`) are logged in one `info` line, `telemetry export stopped`, which reaches the file only.
  - A blocking flush that outlives the bound ends at the exporter timeout (10 s by default) or when the process's runtime drops the batch task.

### gRPC, build time and size

- **HTTP** (`http/protobuf`, `http/json`) uses `opentelemetry-otlp`'s `reqwest-client` on the workspace `reqwest 0.13` and its TLS. No new TLS stack and no blocking client.
  - The client never follows redirects (`redirect::Policy::none()`). reqwest would replay a 307/308 POST with its body and headers, and it strips only the standard auth headers on a host change, so a custom header (`x-api-key`, `dd-api-key`) and the Session data could reach a host that was never configured, in clear text past the https-or-loopback check of the configured endpoint. A 3xx is a failed export. gRPC (tonic) does not follow redirects.
  - An exporter the SDK cannot build (its `http::Uri` parser rejects an endpoint `url::Url` accepts, such as a non-ASCII host) keeps export off: `Telemetry` keeps the reason, with the header values and the endpoint redacted, and `show` reports export and every signal off with it. The SDK reads `OTEL_EXPORTER_OTLP_{,TRACES_,METRICS_,LOGS_}COMPRESSION` itself; built without `gzip-http`/`zstd-http`, version 0.33.1 ignores a value it cannot honor with a warning and exports uncompressed, so the variables never fail the build.
- **gRPC** uses `opentelemetry-otlp`'s `grpc-tonic` with `tonic 0.14` and the `tls-aws-lc` and `tls-native-roots` features, the same `aws-lc-rs` provider as the workspace `rustls`. It sits behind `cyber-telemetry`'s `otlp-grpc` feature, which `cyber-cli` enables by default. `h2`, `hyper 1` and `tower 0.5` are already in the lock file, so the new code is mostly `tonic` and `prost`. `prost` is also needed by `http/protobuf`.
- **Pinning.** Versions are pinned exactly (`=0.33.1`) because the async-runtime processor API is still marked experimental upstream.
- **Verification.** The PR's CI Build matrix builds all 8 release targets. The release-binary size before and after is recorded in this design when the PR's CI has run. If gRPC adds more than 1.5 MiB, the feature becomes opt-in for the musl targets, which are the size-sensitive install path, and `show` reports `grpc` as unsupported there.

### `cyber telemetry show` and `GET /api/v1/telemetry`

- **The route.** `GET /api/v1/telemetry` (`v1.telemetry.get`, new group `telemetry`, owner-authenticated, Location-scoped so `ignored` names the request Location's project layers) returns `TelemetryStatus`. `AppState.telemetry` holds a `TelemetryReport`, which `run_server_until` sets; a host without server listeners (an embedded or in-process router) has none and answers `503` `ServiceUnavailableError` with `service: telemetry`. The route masks every header value and endpoint itself, whatever the report holds:

```
{
  enabled: bool,
  reason?: string,
  log_prompts: { value, source },
  signals: {
    traces|metrics|logs: {
      enabled, endpoint, protocol, timeout_ms,
      headers: [ { name, value: "***" } ],
      sources: { endpoint, protocol, headers, timeout },
      exported, failed, dropped,
      last_error?: { message, at_ms }
    }
  },
  dropped_events,
  resource: { service.name, service.version, os.type, account_signed_in },
  ignored: [ { pointer, source } ],
  restart_required: bool,
  product: { enabled: false }
}
```

- **Source labels:** `config (<layer label>)`, `env (<VARIABLE>)`, `policy (<managed label>)` or `default`.
- **Why the server answers.** The values are the server's own, because the server's environment is the one that exports. The CLI starts the service like every resource command.
- **The command.** `cyber telemetry show` prints the status as a table and supports `--format json`. It exits 0 whether or not export is on.
- **Account values are not echoed.** The account `sub` and org id are reported only as `account_signed_in: true`. `show` is for checking settings, not for revealing identifiers.

## Risks / Trade-offs

- [Spans for long Sessions] → A session span covers one Drain, and `cyber.session.id` ties the periods together. Collectors that want one span per Session lifetime cannot have it. That is the price of spans that end.
- [Child Session parenting after a restart] → It is read once from `SessionInfo`. When the parent's spans died with the restart, the child's spans start a new trace.
- [Experimental SDK runtime API] → Exact pins. If the API changes, the fallback is the SDK's thread-based processors with the blocking `reqwest` client for HTTP, and gRPC on a dedicated single-thread Tokio runtime.
- [SDK queue overflow not counted] → This is documented, the queues are sized above the mapper's bus capacity, and bus lag is counted.
- [Event observation time vs. commit time] → The difference is milliseconds in one process. Hook spans use recorded times.
- [Headers over plain HTTP] → They are refused except on loopback. That may surprise users of in-cluster collectors without TLS, so `show` names the rule.

## Migration

No data migration. Export stays off until `telemetry.otel.enabled: true` is set in global configuration. Existing `telemetry.usage_retention_days` and `telemetry.log_hook_io` keep their meaning.
