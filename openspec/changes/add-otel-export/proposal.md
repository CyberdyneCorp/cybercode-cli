## Why

`observability-costs` → OpenTelemetry export and Prompt content privacy in telemetry (both P1) are specified but not built, and `cyber telemetry` is a command-tree entry that answers "not available". Teams that run agents unattended (workflows, goals, loops) want the same traces, cost metrics and logs in their own collector that they use for every other service. Today the only signals are the local JSON-lines log (`crates/cyber-core/src/log.rs`), the usage ledger and the event store, which a collector cannot read. The sandbox spec also says denials and escalations "SHALL be exported via `observability-costs` telemetry when enabled". That waits for this exporter.

## What Changes

- **Configuration** (`observability-costs`, modified "OpenTelemetry export", added "OpenTelemetry configuration"):
  - Keys: `telemetry.otel.enabled` (default `false`), `telemetry.otel.endpoint`, `telemetry.otel.protocol` (`http/protobuf` default, `http/json`, `grpc`), `telemetry.otel.headers` (secret values) and `telemetry.log_prompts` (default `false`).
  - These keys are read from global configuration only. A project layer cannot redirect export or turn on content capture.
  - The standard `OTEL_*` variables are honored: the generic and per-signal `OTEL_EXPORTER_OTLP_*` endpoint, headers, protocol and timeout, `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none`, `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` and `OTEL_SDK_DISABLED`.
  - The precedence is stated. Environment variables override configuration, as for every other setting. An `OTEL_*` endpoint on its own never turns export on. `OTEL_SDK_DISABLED=true` and `CYBER_DISABLE_TELEMETRY` always turn it off.
  - A managed policy document that cannot be used keeps export off (fail closed). No managed `telemetry` layer exists today (see design).
- **One exporter** (`crates/cyber-telemetry`, new) subscribes to the runtime's live event stream (`Runtime::subscribe`). It maps Session, Turn, tool, permission, hook, sandbox, workflow-run and workflow-agent events to the three signals:
  - **Traces:** spans for Sessions, Turns, tool calls, subagent and workflow-agent child Sessions, workflow runs, workflow agents and hook executions, with defined parenting. A workflow run is the root of its own trace.
  - **Metrics:** tokens by class, cost, Turns, tool calls, tool errors, permission decisions by outcome, Session count and active time.
  - **Logs:** the structured log records of `cyber_core::log` are also exported as OTLP log records through a new log sink.
  - **Resource:** `service.name=cyber`, `service.version`, `os.type`, and the Cyber Account `sub` and org id when signed in.
- **Privacy** (modified "Prompt content privacy in telemetry"):
  - By default no prompt text, model output, tool input or output, Session title, file path or hook IO is exported. Tool spans carry tool name, duration and outcome.
  - With `telemetry.log_prompts: true`, content is exported after the existing secret redaction (`redact_secrets` for structured values, the `redact_credentials` patterns for text), with `***` as the marker.
  - Header values never appear in an attribute, a log line or `cyber telemetry show`.
- **Delivery** (added "OpenTelemetry delivery"):
  - Export never blocks or fails a Turn. Queues are bounded, export is batched, and drops are counted.
  - An unreachable endpoint is reported once, in the log and in `cyber telemetry show`, not once per event.
  - Shutdown ends open spans as interrupted and flushes within 3 seconds.
  - Spans that were unfinished when the server stopped are not resurrected after a restart.
- **`cyber telemetry show`** (`cli-commands`, added "Telemetry status command") reads `GET /api/v1/telemetry` (`server-api`, modified "Route groups": new `telemetry` group). It shows:
  - whether export is on, and why not when it is off,
  - the effective endpoint and protocol for each signal, with headers masked,
  - where each value came from (configuration layer, environment variable, policy or default),
  - export counters (exported, failed, dropped, last error).
- The SDK gains `client.telemetry.get()` from the new operation.

## Non-goals (deferred)

- **Org policy forcing telemetry on or off, and requiring redaction** (`org-policy` → Telemetry policy, P3), beyond not weakening an existing managed layer. Org policy is a P3 capability with its own policy-document schema and server-fetched policy. Today the OS-managed document has no `telemetry` key: `policy.telemetry` is rejected as malformed. This change only makes a malformed managed document keep export off, and leaves a future managed layer free to win over every user setting.
- **Exemplars and histogram bucket tuning.** Every metric in this change is a counter, so there is no histogram to tune. Exemplars need sampled trace context on metric recording, which the event-driven mapper does not have when it records. Both are additive later without changing the metric names.
- **A bundled collector or dashboards.** Cyber exports standard OTLP to whatever collector the user runs. Shipping and versioning a collector distribution or vendor dashboards is packaging work outside the CLI.
- TLS client certificates and custom CA bundles (`OTEL_EXPORTER_OTLP_CERTIFICATE` and related variables) are not read. Export uses the system roots, as the rest of the CLI's HTTPS does.
- Product telemetry (`telemetry.product`) stays unimplemented. `cyber telemetry show` reports it as off.

## Impact

- **Specs:**
  - `observability-costs`: OpenTelemetry export and Prompt content privacy in telemetry are modified; OpenTelemetry configuration and OpenTelemetry delivery are added.
  - `cli-commands`: Telemetry status command is added.
  - `server-api`: Route groups is modified to add the `telemetry` group.
- **Code:**
  - A new crate `crates/cyber-telemetry`: settings resolution, the event-to-signal mapper, pipelines, counters and the log bridge. It pins `opentelemetry`, `opentelemetry_sdk` and `opentelemetry-otlp` at `=0.33.1`, which build on the workspace's `reqwest 0.13` and `tokio 1`.
  - `crates/cyber-core`:
    - `config/telemetry.rs` (new): the typed `telemetry` section.
    - `config/load.rs`: `load_telemetry`, the global layers and managed state with sources.
    - `log.rs`: an optional record sink.
    - `memory/secrets.rs`: redaction with a caller-chosen marker.
  - `crates/cyber-server`:
    - `runtime/bus.rs` and `runtime/hooks.rs`: an internal hook-receipt live event without IO, which client streams never forward.
    - `runtime/requests.rs`: a request the runtime withdraws (a stop, an abandoned call) records its `permission.replied.1` with `cancelled: true`, so it is not counted as a person's decision.
    - `http/telemetry.rs` (new): `GET /api/v1/telemetry`.
    - `http/openapi.rs` and the OpenAPI document.
  - `crates/cyber-app/src/server.rs`: the exporter starts with the server and is flushed at shutdown, next to the retention sweeper. `App::build_served` (used by `cyber serve`) subscribes before the restart recovery; in-process Apps (`App::build`) never export.
  - `crates/cyber-cli`: `commands/telemetry.rs` (new), `tree.rs`, `docs/cli.md`.
  - `sdk/typescript` is regenerated.
- **Build:** HTTP export uses the workspace `reqwest`, with no new TLS stack. gRPC adds `tonic 0.14` on the `aws-lc-rs` provider the workspace already uses, behind the `otlp-grpc` feature of `cyber-telemetry`, which `cyber-cli` enables. The PR's CI builds all 8 release targets. The design records the size increase measured there.
- **Docs:** the local core guide gets an "OpenTelemetry export" section, plus the feature reference and README rows. `docs/local-core-guide.md`'s note that sandbox telemetry "waits for the telemetry exporter" is resolved.
