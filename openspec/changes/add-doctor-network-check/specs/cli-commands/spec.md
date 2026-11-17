## ADDED Requirements

### Requirement: Doctor network check
(P1) `cyber doctor --network` SHALL run only network checks, as pass/warn/fail lines, and SHALL exit 1 when any fails:
- `proxy`: the `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` values in effect (either case), with credentials in proxy URLs replaced by `***`, or `none set`.
- `network <provider>`: one line per enabled catalog provider with a URL that has a key or a subscription login, and per endpoint configured under `providers` that needs no credential. The probe SHALL use the provider HTTP client settings, send no credentials, follow no redirects and time out after 10 seconds. Any HTTP response SHALL pass with its status and latency. A DNS, connection, TLS or timeout error SHALL fail, or warn for an endpoint that needs no credential. Details SHALL name the host and port, never a URL path or query.
- `network updates`: the same probe of the build channel's update manifest (the stable channel for development builds); it SHALL warn when unreachable.

Probes SHALL run concurrently. `cyber doctor` without `--network` SHALL keep its checks unchanged.

#### Scenario: Provider blocked by the network
- **WHEN** `OPENAI_API_KEY` is set, `api.openai.com` refuses connections and the user runs `cyber doctor --network`
- **THEN** the `network openai` line reports `fail` with `api.openai.com unreachable: cannot connect (…)`, no request carried the key, and the exit code is 1

#### Scenario: Local server not running
- **WHEN** `providers.ollama` configures `http://127.0.0.1:11434/v1` with `auth: none` and nothing listens there
- **THEN** the `network ollama` line reports `warn`
