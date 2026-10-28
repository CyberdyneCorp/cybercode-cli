## ADDED Requirements

### Requirement: Model behavior profiles
(P1) The system SHALL resolve a behavior profile for every model. A profile SHALL have these fields:
- `reasoning_replay`: `never`, `tool_loop` or `always`;
- `reasoning_carrier`: `none`, `responses`, `anthropic`, `reasoning_content`, `reasoning`, `reasoning_details`, `gemini_signature` or `copilot_opaque`;
- `reasoning_summary`: a boolean;
- `verbosity`: `{ supported, default }`;
- `parallel_tool_calls`: a boolean;
- `edit_tool`: `freeform_patch`, `json_patch` or `edit`;
- `tool_output`: `{ tokens, truncation: head | tail | middle }`;
- `effort_clamp`: `nearest_lower` or `fail`.

Profiles SHALL be resolved field by field, with a later layer winning:
1. built-in family defaults;
2. built-in rules matched by model-ID prefix, optionally scoped to a provider;
3. provider catalog metadata, which includes the ChatGPT `/models` fields `support_verbosity`, `default_verbosity`, `supports_reasoning_summary_parameter`, `apply_patch_tool_type`, `truncation_policy` and `supported_reasoning_levels`;
4. config `providers.<provider>.models.<model>.profile`.

An invalid config value SHALL be ignored with a warning naming the key. The resolved profile SHALL appear as `profile` in `cyber models --json` and `GET /api/v1/models`.

An effort that does not apply to the model and comes from an agent, workflow tier or role SHALL be clamped to the nearest lower supported level when `effort_clamp` is `nearest_lower`, with one `info` log line. An explicit user variant SHALL still fail with `VariantUnavailableError`.

#### Scenario: Built-in rule for a compatible provider
- **WHEN** a custom `openai-compatible` provider serves `kimi-k2-thinking` and config sets no profile
- **THEN** the profile has `reasoning_carrier: reasoning_content` and `reasoning_replay: tool_loop`

#### Scenario: Config overrides a field
- **WHEN** config sets `providers.corp.models.coder.profile = { reasoning_replay: "never" }`
- **THEN** `cyber models corp --json` shows `reasoning_replay: "never"`, and the other fields keep their built-in values

#### Scenario: Effort clamped for a workflow tier
- **WHEN** a workflow tier asks for `xhigh` on a model whose levels are `low`, `medium` and `high`
- **THEN** the request uses `high`, and one `info` line names both levels

### Requirement: Responses request shaping
(P1) Requests through the OpenAI Responses protocol (OpenAI, the ChatGPT subscription provider, and Copilot models served by Responses) SHALL:
- send `include: ["reasoning.encrypted_content"]` when the model has the `reasoning` capability;
- send `reasoning.summary: "auto"` only when the profile has `reasoning_summary`;
- send `reasoning.effort: "none"` for reasoning off only when the model lists `none`, and otherwise leave `reasoning` out;
- send `text.verbosity` with the profile's default only when verbosity is supported;
- send `tool_choice: "auto"` and `parallel_tool_calls` from the profile whenever tools are offered;
- send `prompt_cache_key` derived from the root Session ID (`<root>` for the Session, `<root>:<agent>` for a subagent) on every request unless caching is turned off.

Reasoning output items SHALL be stored as reasoning items (`session-runtime` → Reasoning items) and replayed in their original position. An item ID the provider did not issue SHALL be left out of the replayed item.

#### Scenario: Request fields for a reasoning model
- **WHEN** a Turn on `openai/gpt-6-sol` (verbosity supported, default `low`) offers tools
- **THEN** the body has `store: false`, `include: ["reasoning.encrypted_content"]`, `text.verbosity: "low"`, `tool_choice: "auto"`, `parallel_tool_calls: true` and `prompt_cache_key` equal to the Session ID

#### Scenario: Summary left out where unsupported
- **WHEN** the profile has `reasoning_summary: false`
- **THEN** `reasoning` holds only `effort`

### Requirement: Session routing headers
(P1) Requests to the ChatGPT subscription backend SHALL carry:
- `session-id`: the root Session ID;
- `thread-id`: the Session ID;
- `x-client-request-id`: the Session ID;
- `x-openai-subagent` for non-Turn calls: `compact`, `title` or `recap`, or for a subagent, the agent name.

The first `x-codex-turn-state` response header of a Turn SHALL be sent back on every later request and retry of that Turn, and SHALL be forgotten when the Turn ends. It SHALL never be persisted. Requests to the OpenAI API with an API key SHALL carry `session-id` only. Other providers SHALL receive none of these headers.

#### Scenario: Turn state echoed within a Turn only
- **WHEN** the first response of a Turn returns `x-codex-turn-state: abc`, the Turn makes two more requests, and a new Turn starts
- **THEN** both follow-up requests send `x-codex-turn-state: abc`, and the first request of the new Turn sends none

### Requirement: Stream idle timeout
(P1) Every streaming adapter SHALL fail a response stream that delivers no bytes for `provider.timeout.idle_ms` (default 300000; `0` turns it off) with a `Transport` error `stream idle for <n>s`. Retry Policy SHALL then treat it like any other transport failure: retried when no output was received, otherwise handled as a mid-stream failure.

#### Scenario: Stalled stream
- **WHEN** a provider sends headers and one event, then nothing for 300 seconds
- **THEN** the step fails with `Transport` `stream idle for 300s`, and the partial output prevents automatic replay

### Requirement: Provider-specific error codes
(P1) The system SHALL map known provider error codes, from in-stream errors and HTTP bodies, onto the existing typed reasons:
- `context_length_exceeded` → `ContextOverflow`;
- `insufficient_quota`, `credit_balance_exhausted` and the `*_spend_limit_exceeded` codes → `QuotaExceeded`;
- `usage_limit_reached` → `QuotaExceeded`, carrying `resets_at` and `plan_type`;
- `usage_not_included` → `Authentication`, with the message "plan does not include this model";
- `server_is_overloaded`, `slow_down` and `policy_capacity_unavailable` → `ProviderInternal`;
- `rate_limit_exceeded` → `RateLimit`;
- `invalid_prompt` and the `*_policy` codes → `ContentPolicy`.

A server-advised delay SHALL be taken from `retry-after-ms`, then `Retry-After`, then a message matching `try again in <n>(ms|s|seconds)`. Retry Policy SHALL use that delay, capped at 60 seconds, in place of its own backoff.

A subscription usage limit SHALL be shown in the TUI and in `exec` as `usage limit reached; resets at <local time>`.

A Responses `response.incomplete` SHALL finish as follows:
- `interrupted` → `Stop`;
- `max_output_tokens` → `Length`;
- `content_filter` → `ContentFilter`;
- any other reason → the step fails as `ProviderInternal` with that reason.

#### Scenario: Overload with an advised delay
- **WHEN** a stream fails with code `server_is_overloaded` and message "Please try again in 2.5s"
- **THEN** the reason is `ProviderInternal`, and the retry waits 2.5 seconds

#### Scenario: Subscription limit reached
- **WHEN** the ChatGPT backend answers 429 with `error.type: "usage_limit_reached"` and `resets_at` in 40 minutes
- **THEN** the reason is `QuotaExceeded`, fallback models are tried, and with none available the TUI shows `usage limit reached; resets at <time>`
