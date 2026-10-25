## ADDED Requirements

### Requirement: Telemetry status command
(P1) `cyber telemetry show` SHALL print the running server's telemetry status from `GET /api/v1/telemetry`. For each signal (traces, metrics, logs) it SHALL show:

- whether OpenTelemetry export is on and, when off, why,
- the effective endpoint (without userinfo or query), protocol and timeout,
- the header names with every value shown as `***`,
- the source of each value (`config (<layer>)`, `env (<VARIABLE>)`, `policy (<managed source>)` or `default`),
- the counters `exported`, `failed` and `dropped`, and the last export error with its time.

It SHALL also show:

- `telemetry.log_prompts` and its source,
- the configuration values it ignored (project layers),
- whether the configuration changed since the server started (a restart is required),
- whether product telemetry is on.

`--format json` SHALL print the same status as JSON. The command SHALL exit 0 whether or not export is on. Header values and Cyber Account identifiers SHALL NOT be printed.

#### Scenario: Masked headers and sources
- **WHEN** global configuration enables export with `headers: { "authorization": "Bearer abc" }` and `OTEL_EXPORTER_OTLP_PROTOCOL=http/json` is set on the server
- **THEN** `cyber telemetry show` lists `authorization=***` with source `config`, protocol `http/json` with source `env (OTEL_EXPORTER_OTLP_PROTOCOL)`, and the output contains no `abc`

#### Scenario: Export off by default
- **WHEN** no telemetry configuration exists and the user runs `cyber telemetry show --format json`
- **THEN** the JSON has `enabled: false` with a reason, every counter is 0, and the exit code is 0
