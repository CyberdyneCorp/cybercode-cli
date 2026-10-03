# provider-catalog Specification

## Purpose
The provider catalog is the Location-scoped registry of LLM providers and models that Cyber Code can use, together with the provider-neutral LLM layer that streams one Turn. It combines OpenCode's models.dev catalog and any-provider design with Claude Code's model roles (advisor, fast, effort) and Codex's local-model ergonomics. Cyber Code stays model-agnostic: every model, including models mixed inside one Workflow, goes through the same adapter contract.

## Requirements

### Requirement: Native provider adapters
(P0) The system SHALL ship native Rust adapters for the OpenAI Responses API, OpenAI-compatible Chat Completions, Anthropic Messages, local servers (Ollama, llama.cpp server, vLLM via the OpenAI-compatible adapter). Every adapter SHALL emit the same provider-neutral stream event type (`text.delta`, `reasoning.delta`, `tool_call.delta`, `tool_call.done`, `usage`, `finish`, `error`).

#### Scenario: Same events from different providers
- **WHEN** one Turn streams from `anthropic/claude-sonnet` and another from `openai/gpt-6`
- **THEN** the session runtime receives the same `LlmEvent` variants from both adapters
- **AND** no provider-specific type crosses the adapter boundary

#### Scenario: Local server through the compatible adapter
- **WHEN** a provider is configured with `api.type: "openai-compatible"` and `api.url: "http://127.0.0.1:11434/v1"`
- **THEN** the system streams Turns from that server without requiring an API key

### Requirement: Provider and model records
(P0) The system SHALL represent providers as `{ id, name, api: { type, url?, settings }, request: { headers, body }, disabled? }` and models as `{ id, provider_id, name, family?, api_id, capabilities, limits: { context, input?, output }, cost, variants, status, released_at }`. `capabilities` SHALL cover `tools`, `vision`, `pdf`, `reasoning`, `structured_output`, `prompt_cache`, and `temperature`.

#### Scenario: Capabilities drive tool exposure
- **WHEN** a model's `capabilities.tools` is false
- **THEN** the session runtime uses the tool emulation path instead of sending native tool definitions

### Requirement: models.dev catalog source and cache
(P0) The system SHALL load provider metadata from `<CYBER_MODELS_URL or https://models.dev>/api.json`, cache it at `~/.cache/cyber/models.json` (`models-<hash>.json` for non-default URLs), treat the cache as fresh for 5 minutes, refresh it in the background every 60 minutes, fetch with a 10-second timeout and 2 transient retries, and serialize writers with a cross-process file lock.

#### Scenario: Fresh cache avoids network
- **WHEN** the cache file was written 3 minutes ago
- **THEN** the catalog loads from the cache without a network request

#### Scenario: Concurrent refresh
- **WHEN** two `cyber` processes refresh the catalog at the same time
- **THEN** only one writes the cache file and the other re-reads the result after acquiring the lock

### Requirement: Offline and pinned catalog
(P0) The system SHALL read the catalog from `CYBER_MODELS_PATH` when set, fall back to a snapshot bundled into the binary when no cache exists, and make no catalog network request when `CYBER_DISABLE_MODELS_FETCH` is truthy.

#### Scenario: Air-gapped start
- **WHEN** the machine has no network, no cache, and `CYBER_DISABLE_MODELS_FETCH=1`
- **THEN** the catalog is built from the bundled snapshot plus configured providers

### Requirement: Configured providers and models
(P0) The system SHALL apply the `providers` config after catalog data in config layer order, merging request headers and body, replacing `api` fields that are set, adding custom models with explicit capabilities and limits, and accepting `disabled: true` on a provider or model. A model absent from the catalog SHALL default to text in/out, tools enabled, unknown pricing (`cost: null`, `unpriced`), and limits taken from config.

#### Scenario: Custom OpenAI-compatible provider
- **WHEN** config defines `providers.corp = { api: { type: "openai-compatible", url: "https://llm.corp/v1" }, models: { "coder-7b": { limits: { context: 32768, output: 4096 } } } }`
- **THEN** `corp/coder-7b` is listed and selectable

### Requirement: Model reference format
(P0) The system SHALL identify models as `provider/model[#variant]`, splitting at the first `/` so model IDs may contain slashes, and at the last `#` for the variant, in config, agent definitions, CLI flags, workflow `agent()` options, and HTTP payloads.

#### Scenario: Model ID containing slashes and a variant
- **WHEN** the user passes `--model openrouter/meta/llama-4#high`
- **THEN** the provider is `openrouter`, the model is `meta/llama-4`, and the variant is `high`

### Requirement: Availability
(P0) The system SHALL treat a provider as available when it is not disabled, is allowed by org policy, and has a resolvable credential (or requires none), and a model as available when it is enabled, its status is not `deprecated`, and its provider is available. Models with status `alpha` SHALL be hidden unless `CYBER_ENABLE_EXPERIMENTAL_MODELS` is truthy.

#### Scenario: Provider without credentials
- **WHEN** `anthropic` has no API key, OAuth credential, or environment variable
- **THEN** its models are listed as unavailable with reason `missing_credentials`

### Requirement: Model roles
(P0) The system SHALL resolve the roles `default`, `small`, `advisor`, `evaluator`, `compaction`, and `title` from `model_roles` config, using the canonical configuration fallback: `default` uses `model`; `small` uses `small_model` then `default`; `title` and `compaction` use `small` then `default`; `evaluator` uses `small` then `default`; an unset `advisor` is disabled. Workflows MAY define named tiers under `workflows.tiers` (for example `fast`, `strong`) that resolve to model references.

#### Scenario: Title uses the small role
- **WHEN** `model_roles.title` is unset and `small_model` is `openai/gpt-6-mini`
- **THEN** session titles are generated with `openai/gpt-6-mini`

### Requirement: Default model resolution
(P0) The system SHALL choose the default model as the configured `model` when available, otherwise the most recently used available model recorded in `~/.local/state/cyber/model.json`, otherwise the newest available model by `released_at`, and SHALL fail with `ModelNotSelectedError` when none is available.

#### Scenario: Configured model unavailable
- **WHEN** config sets `model: "anthropic/claude-opus"` but Anthropic has no credentials and the last used model is `openai/gpt-6`
- **THEN** the default model is `openai/gpt-6`
- **AND** a warning names the unavailable configured model

### Requirement: Small model selection
(P1) When `cyber models --suggest-small` is invoked, the system SHALL suggest a small model from the default model's provider among active, tool-capable models released within 18 months, preferring names containing `mini`, `nano`, `flash`, `haiku`, `lite`, `small`, or `fast` and ranking by 80% relative cost plus 20% relative age, falling back to the default model. Suggestions SHALL NOT change role configuration without explicit user selection.

#### Scenario: Suggested small model
- **WHEN** the default provider offers `gpt-6` and `gpt-6-mini`
- **THEN** the command suggests `gpt-6-mini` and the effective small role remains unchanged until selected

### Requirement: Variants and effort
(P0) The system SHALL expose reasoning variants per model (`minimal`, `low`, `medium`, `high`, `xhigh`, `max` as supported) derived from catalog reasoning options or per-adapter rules, merge configured variants by ID, and remove variants marked `disabled: true`. An unknown explicit variant SHALL fail with `VariantUnavailableError`.

#### Scenario: Effort switch
- **WHEN** the user runs `/model anthropic/claude-sonnet#high`
- **THEN** subsequent Turns send the adapter's high-effort reasoning parameters

### Requirement: Request option layering
(P0) The system SHALL build each request by merging, in order, adapter defaults, provider `request`, model `request`, variant `request`, agent `request`, and per-call overrides, deep-merging bodies per provider namespace and merging headers. `apiKey` SHALL never be sent in a request body.

#### Scenario: Agent overrides temperature
- **WHEN** the provider sets `temperature: 0.7` and the agent sets `temperature: 0.2`
- **THEN** the request carries `temperature: 0.2`

### Requirement: Error classification
(P0) The system SHALL map provider failures to typed reasons: `Authentication` (401/403), `RateLimit` and `QuotaExceeded` (429), `ContextOverflow` (detected from status 400/413/422 or message patterns), `ContentPolicy`, `InvalidRequest` (other 4xx), `ProviderInternal` (5xx, 529), and `Transport`. Error diagnostics SHALL be redacted of credentials.

#### Scenario: Overflow detection
- **WHEN** a provider returns 400 with a message matching "maximum context length"
- **THEN** the error reason is `ContextOverflow`

### Requirement: Retry policy
(P0) The system SHALL retry `RateLimit`, `ProviderInternal`, and `Transport` failures up to 4 times, waiting `retry-after-ms` or `retry-after` when present (capped at 60 seconds) and otherwise using jittered exponential backoff starting at 1 second (factor 2, cap 30 seconds), and SHALL never retry `ContextOverflow`, `Authentication`, `ContentPolicy`, or `InvalidRequest`. Each retry SHALL publish a live `session.retry` event with the attempt number and delay. Automatic request retries SHALL apply only before assistant output or tool dispatch. After partial output or dispatch, the Turn SHALL be settled as interrupted and tool outcomes reconciled before a new Turn; the system SHALL NOT replay the original request as though no work happened.

#### Scenario: Retry-after honored
- **WHEN** a 429 response carries `retry-after: 7`
- **THEN** the next attempt starts after 7 seconds

#### Scenario: Overflow not retried
- **WHEN** the error reason is `ContextOverflow`
- **THEN** no retry is attempted and the compaction capability decides what happens next

### Requirement: Prompt caching
(P0) The system SHALL keep the system prompt baseline and tool definitions byte-stable within a Context Epoch, place provider cache breakpoints after them where the adapter supports explicit caching, and send a cache key derived from the Session ID where the adapter supports keyed caching (for example OpenAI `prompt_cache_key`). `cache: "none"` on a request SHALL disable caching hints.

#### Scenario: Stable prefix across Turns
- **WHEN** two consecutive Turns run in the same Context Epoch
- **THEN** the serialized system and tool prefix is byte-identical in both requests

### Requirement: Tool-call emulation for models without native tools
(P1) The system SHALL, for models whose `capabilities.tools` is false, describe tools in the system prompt and parse a fenced JSON block `{"tool": "<name>", "input": {...}}` from the response as a tool call, rejecting malformed blocks with a model-visible error and allowing at most one emulated call per Turn.

#### Scenario: Local model calls a tool
- **WHEN** a local model without native tools replies with a valid emulated tool block for `read`
- **THEN** the runtime executes `read` and sends the result back as a user-role tool result message

### Requirement: Advisor tool
(P2) When `model_roles.advisor` resolves to a model different from the Turn's model, the system SHALL offer an `advisor` tool that sends a question plus a compact transcript summary to the advisor model with no tools and returns its answer as tool output. Advisor usage SHALL be accounted separately in session cost.

#### Scenario: Consulting a stronger model
- **WHEN** a session running `ollama/qwen-coder` calls `advisor` with a design question and the advisor role is `anthropic/claude-opus`
- **THEN** the answer comes from `anthropic/claude-opus` and its cost is recorded under `advisor`

### Requirement: Cost accounting
(P0) The system SHALL compute per-step cost from usage as input, output, reasoning (billed at the output price unless the catalog prices it separately), cache-read, and cache-write tokens multiplied by the model's per-million prices, applying context-tier pricing when input exceeds a tier threshold, and SHALL record `cost: null` with `unpriced` for unknown pricing, never zero. Explicitly configured zero pricing SHALL represent a known zero API charge; hardware and electricity costs are outside this accounting.

#### Scenario: Cached tokens priced separately
- **WHEN** a step reports 10,000 input tokens of which 8,000 are cache reads
- **THEN** 2,000 tokens are billed at the input price and 8,000 at the cache-read price

### Requirement: Fast mode
(P2) The system SHALL support a per-session `fast` toggle that selects a provider's fast service tier or a configured fast variant when the catalog exposes one, and SHALL report `fast unavailable` for models without one.

#### Scenario: Toggle fast mode
- **WHEN** the user runs `/fast` on a model with a fast tier
- **THEN** subsequent Turns request the fast tier and the status line shows `fast`

### Requirement: Models command and API
(P0) The system SHALL provide `cyber models [provider] [--verbose] [--refresh] [--json]` printing one `provider/model` per line (available first, then unavailable with reason), and `GET /api/v1/models` and `GET /api/v1/providers` returning the same data, including the resolved model roles.

#### Scenario: Unknown provider argument
- **WHEN** the user runs `cyber models nope`
- **THEN** the command fails with `Provider not found: nope` and exit code 1

### Requirement: Catalog change events
(P0) The system SHALL publish `catalog.updated` after every catalog rebuild (refresh, config reload, credential change, policy change), and clients SHALL refresh model pickers on that event.

#### Scenario: Login makes models available
- **WHEN** the user stores an OpenAI API key
- **THEN** `catalog.updated` is published and OpenAI models become available without a restart

### Requirement: Additional native provider adapters
(P1) The system SHALL add native adapters for Google Gemini, Amazon Bedrock Converse, Google Vertex AI and Azure OpenAI. Each SHALL pass the same provider-neutral stream, recovery and live-quality evaluation contracts before being listed as supported.

#### Scenario: New adapter release gate
- **WHEN** a Gemini adapter is added
- **THEN** it passes the provider contract fixtures and publishes its own live coding baseline before release

### Requirement: Model fallback chain
(P1) `fallback_models` SHALL be an ordered list of model refs tried after the Session model when a Turn fails with `RateLimit`, `QuotaExceeded`, `ProviderInternal` or `Transport` after the retry policy is exhausted, or immediately with `Authentication` or `ModelDeniedByPolicyError`. The failed request SHALL be re-sent on the next available model before any assistant output or tool dispatch, with reasoning lowering applied, and a durable `session.model.fallback.1` event SHALL record the reason and both models. The Session SHALL return to its configured model after `fallback.sticky_minutes` (default 10) or on the user's next explicit model switch. The status line SHALL show the active fallback. Agents and workflow tiers MAY set their own `fallback_models`. Fallback SHALL never select a model denied by org policy.

#### Scenario: Provider overloaded
- **WHEN** the primary model returns 529 four times and `fallback_models` is `["anthropic/claude-sonnet", "openai/gpt-6"]`
- **THEN** the Turn is re-sent on `anthropic/claude-sonnet`, the event records the fallback, and the footer shows `fallback: anthropic/claude-sonnet`
