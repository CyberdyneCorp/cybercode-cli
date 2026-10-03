## ADDED Requirements

### Requirement: Additional native provider adapters
(P1) The system SHALL add native adapters for Google Gemini, Amazon Bedrock Converse, Google Vertex AI and Azure OpenAI. Each SHALL pass the same provider-neutral stream, recovery and live-quality evaluation contracts before being listed as supported.

#### Scenario: New adapter release gate
- **WHEN** a Gemini adapter is added
- **THEN** it passes the provider contract fixtures and publishes its own live coding baseline before release

## MODIFIED Requirements

### Requirement: Native provider adapters
(P0) The system SHALL ship native Rust adapters for the OpenAI Responses API, OpenAI-compatible Chat Completions, Anthropic Messages, local servers (Ollama, llama.cpp server, vLLM via the OpenAI-compatible adapter). Every adapter SHALL emit the same provider-neutral stream event type (`text.delta`, `reasoning.delta`, `tool_call.delta`, `tool_call.done`, `usage`, `finish`, `error`).

#### Scenario: Same events from different providers
- **WHEN** one Turn streams from `anthropic/claude-sonnet` and another from `openai/gpt-6`
- **THEN** the session runtime receives the same `LlmEvent` variants from both adapters
- **AND** no provider-specific type crosses the adapter boundary

#### Scenario: Local server through the compatible adapter
- **WHEN** a provider is configured with `api.type: "openai-compatible"` and `api.url: "http://127.0.0.1:11434/v1"`
- **THEN** the system streams Turns from that server without requiring an API key

### Requirement: Configured providers and models
(P0) The system SHALL apply the `providers` config after catalog data in config layer order, merging request headers and body, replacing `api` fields that are set, adding custom models with explicit capabilities and limits, and accepting `disabled: true` on a provider or model. A model absent from the catalog SHALL default to text in/out, tools enabled, unknown pricing (`cost: null`, `unpriced`), and limits taken from config.

#### Scenario: Custom OpenAI-compatible provider
- **WHEN** config defines `providers.corp = { api: { type: "openai-compatible", url: "https://llm.corp/v1" }, models: { "coder-7b": { limits: { context: 32768, output: 4096 } } } }`
- **THEN** `corp/coder-7b` is listed and selectable

### Requirement: Model roles
(P0) The system SHALL resolve the roles `default`, `small`, `advisor`, `evaluator`, `compaction`, and `title` from `model_roles` config, using the canonical configuration fallback: `default` uses `model`; `small` uses `small_model` then `default`; `title` and `compaction` use `small` then `default`; `evaluator` uses `small` then `default`; an unset `advisor` is disabled. Workflows MAY define named tiers under `workflows.tiers` (for example `fast`, `strong`) that resolve to model references.

#### Scenario: Title uses the small role
- **WHEN** `model_roles.title` is unset and `small_model` is `openai/gpt-6-mini`
- **THEN** session titles are generated with `openai/gpt-6-mini`

### Requirement: Small model selection
(P1) When `cyber models --suggest-small` is invoked, the system SHALL suggest a small model from the default model's provider among active, tool-capable models released within 18 months, preferring names containing `mini`, `nano`, `flash`, `haiku`, `lite`, `small`, or `fast` and ranking by 80% relative cost plus 20% relative age, falling back to the default model. Suggestions SHALL NOT change role configuration without explicit user selection.

#### Scenario: Suggested small model
- **WHEN** the default provider offers `gpt-6` and `gpt-6-mini`
- **THEN** the command suggests `gpt-6-mini` and the effective small role remains unchanged until selected

### Requirement: Retry policy
(P0) The system SHALL retry `RateLimit`, `ProviderInternal`, and `Transport` failures up to 4 times, waiting `retry-after-ms` or `retry-after` when present (capped at 60 seconds) and otherwise using jittered exponential backoff starting at 1 second (factor 2, cap 30 seconds), and SHALL never retry `ContextOverflow`, `Authentication`, `ContentPolicy`, or `InvalidRequest`. Each retry SHALL publish a live `session.retry` event with the attempt number and delay. Automatic request retries SHALL apply only before assistant output or tool dispatch. After partial output or dispatch, the Turn SHALL be settled as interrupted and tool outcomes reconciled before a new Turn; the system SHALL NOT replay the original request as though no work happened.

#### Scenario: Retry-after honored
- **WHEN** a 429 response carries `retry-after: 7`
- **THEN** the next attempt starts after 7 seconds

#### Scenario: Overflow not retried
- **WHEN** the error reason is `ContextOverflow`
- **THEN** no retry is attempted and the compaction capability decides what happens next

### Requirement: Cost accounting
(P0) The system SHALL compute per-step cost from usage as input, output, reasoning (billed at the output price unless the catalog prices it separately), cache-read, and cache-write tokens multiplied by the model's per-million prices, applying context-tier pricing when input exceeds a tier threshold, and SHALL record `cost: null` with `unpriced` for unknown pricing, never zero. Explicitly configured zero pricing SHALL represent a known zero API charge; hardware and electricity costs are outside this accounting.

#### Scenario: Cached tokens priced separately
- **WHEN** a step reports 10,000 input tokens of which 8,000 are cache reads
- **THEN** 2,000 tokens are billed at the input price and 8,000 at the cache-read price
