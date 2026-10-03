## ADDED Requirements

### Requirement: Request interception
(P2) A plugin declaring the capability `provider.intercept` SHALL receive, over the host protocol, `provider/request` (may rewrite `headers` and `body` before a provider call), `provider/response` (observes status, headers and usage after the call), `provider/retry` (may return `{ retry: boolean, delay_ms? }` for a classified failure) and `session/context` (may contribute a Context Source value under its plugin key). Interceptors SHALL run in plugin load order with a 2-second timeout each; a timeout or error SHALL be non-blocking (the original request proceeds) unless the plugin manifest sets `fail_closed: true`. Interceptors SHALL NOT see or alter credentials: `Authorization` and provider key headers are attached after interception. The capability SHALL be shown before install and MAY be denied by org policy (`plugins.deny_capabilities`).

#### Scenario: Gateway plugin adds tracing headers
- **WHEN** an interceptor returns `{ headers: { "x-trace-id": "abc" } }` from `provider/request`
- **THEN** the provider call carries that header, and the plugin never receives the provider API key
