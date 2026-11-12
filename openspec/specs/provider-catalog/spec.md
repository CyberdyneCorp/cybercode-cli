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
(P0) The system SHALL treat a provider as available when it is not disabled, is allowed by org policy, and has a resolvable credential (or requires none), and a model as available when it is enabled, its status is not `deprecated`, and its provider is available. Models with status `alpha` SHALL be hidden unless `CYBER_ENABLE_EXPERIMENTAL_MODELS` is truthy. Unavailable models SHALL report exactly one reason, checked in this order: `provider_disabled`, `disabled`, `denied_by_policy`, `adapter_unsupported` (the provider's protocol has no native adapter in this build; P0 maps `@ai-sdk/openai` to `openai-responses`, `@ai-sdk/anthropic` to `anthropic`, and `@ai-sdk/openai-compatible` and `@openrouter/ai-sdk-provider` to `openai-compatible`), `missing_url`, `missing_credentials`, `deprecated` and `experimental`.

#### Scenario: Provider without credentials
- **WHEN** `anthropic` has no API key, OAuth credential, or environment variable
- **THEN** its models are listed as unavailable with reason `missing_credentials`

#### Scenario: Provider without a native adapter
- **WHEN** the catalog lists a provider whose SDK has no P0 adapter, such as `@ai-sdk/google`
- **THEN** its models are listed as unavailable with reason `adapter_unsupported` until the P1 adapter ships

### Requirement: Model roles
(P0) The system SHALL resolve the roles `default`, `small`, `advisor`, `evaluator`, `compaction`, and `title` from `model_roles` config, using the canonical configuration fallback: `default` uses `model`; `small` uses `small_model` then `default`; `title` and `compaction` use `small` then `default`; `evaluator` uses `small` then `default`; an unset `advisor` is disabled. Workflows MAY define named tiers under `workflows.tiers` (for example `fast`, `strong`) that resolve to model references.

#### Scenario: Title uses the small role
- **WHEN** `model_roles.title` is unset and `small_model` is `openai/gpt-6-mini`
- **THEN** session titles are generated with `openai/gpt-6-mini`

### Requirement: Default model resolution
(P0) The system SHALL choose the default model as the configured `model` when available, otherwise the most recently used available model recorded in `~/.local/state/cyber/model.json`, otherwise the newest available model by `released_at` among providers with a credential or configured under `providers`, and SHALL fail with `ModelNotSelectedError` when none is available. A keyless local endpoint known only from the catalog (a loopback URL such as LM Studio's) SHALL NOT be chosen automatically, since nothing shows it is running; a configured `model`, a recent model or a `providers` entry selects it. The error, and a Session created without a model, SHALL say how to set up: an API key variable, `cyber providers login chatgpt` or `copilot`, or a `model` in `cyber.jsonc`.

#### Scenario: First run without a provider
- **WHEN** a user with no API key, login or `model` runs `cyber exec "say hi"`
- **THEN** no local endpoint from the catalog is chosen, and the run exits 2 naming `OPENAI_API_KEY`, `cyber providers login chatgpt` and `model` in `cyber.jsonc`

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

#### Scenario: Effort-only Anthropic models
- **WHEN** a Turn uses a variant that resolves to an effort level on the Anthropic adapter (catalog `reasoning_options` of type `effort` only, such as Claude Sonnet 5.5)
- **THEN** the request carries `thinking: {"type": "adaptive"}` and `output_config.effort`, never `budget_tokens`
- **AND** variants that resolve to a token budget still send `thinking: {"type": "enabled", "budget_tokens": N}`

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
(P0) The system SHALL retry `RateLimit`, `ProviderInternal`, and `Transport` failures up to 4 times, waiting `retry-after-ms` or `retry-after` when present (capped at 60 seconds) and otherwise using jittered exponential backoff starting at 1 second (factor 2, cap 30 seconds), and SHALL never retry `ContextOverflow`, `Authentication`, `ContentPolicy`, or `InvalidRequest`. Each retry SHALL publish a live `session.retry` event with the step's `message_id`, the attempt number, the delay, the error and `discarded`. Retries before and after assistant output SHALL share these 4 retries.

A retryable failure after partial output SHALL discard that attempt and send the same request again, with `discarded: true`. Partial output covers text, reasoning, streamed tool-call arguments and complete tool calls of the answer. This replay SHALL apply only before protocol completion and before any tool call of the answer has been dispatched. The idle timeout and in-stream overload errors are such failures. The runtime SHALL NOT dispatch a tool call of an answer while its stream can still be replayed. Calls SHALL dispatch after the stream completes, and starting a call early (`tool-registry` → Eager execution during streaming) SHALL end replay for that answer.

No event of a discarded attempt SHALL be recorded as part of the answer. Reasoning items, text and tool calls SHALL be recorded only when the stream completes or fails for good. A restart, the next request and every client SHALL see only the attempt that ended the step, with reasoning items numbered from 0.

Usage that a discarded attempt reported SHALL be recorded as a failed `usage.recorded.1`, with purpose `turn` and the step's message ID as `call_id`, unless a budget reservation already billed that attempt. It SHALL count toward Session totals and budgets, but not toward the step's own usage or the context meter.

Each attempt SHALL pass the same budget checks and reservation admission as a new request, and no attempt SHALL hold a reservation while another attempt of the same step does. The Turn's `x-codex-turn-state` value SHALL be kept across the replay.

Clients SHALL drop the live text, reasoning and tool input of `message_id` when they receive `discarded: true`.

When retries are exhausted, or a failure after partial output is not retryable, the Turn SHALL be settled as interrupted with the final attempt's partial output. Undispatched calls SHALL be settled as interrupted, and tool outcomes SHALL be reconciled before a new Turn. The system SHALL NOT replay a request after any of its tool calls was dispatched or after protocol completion.

#### Scenario: Retry-after honored
- **WHEN** a 429 response carries `retry-after: 7`
- **THEN** the next attempt starts after 7 seconds

#### Scenario: Overflow not retried
- **WHEN** the error reason is `ContextOverflow`
- **THEN** no retry is attempted and the compaction capability decides what happens next

#### Scenario: HTTP body fails after protocol completion
- **WHEN** a provider stream has emitted its protocol completion, including final usage
- **AND** the HTTP body stalls or closes incorrectly afterward
- **THEN** the completed response remains successful without waiting for the HTTP tail or replaying the request

#### Scenario: Body interruption before protocol completion
- **WHEN** an HTTP response body fails before protocol completion
- **THEN** the transport diagnostic identifies the response-stream stage and received body-byte count without adding response content or credentials
- **AND** failures before assistant output are retried within the bounded retry policy
- **AND** after partial text, reasoning, tool arguments or completed but undispatched tool calls, the attempt is discarded and the same request is replayed within the same budget
- **AND** once the budget is spent, the final attempt's partial output is kept, its undispatched calls are settled as interrupted, and completed tool outcomes stay settled across restart and explicit continuation

#### Scenario: Partial answer replaced by the replay
- **WHEN** a Turn streams "Here is the first half" and a completed `write` call, and then the connection resets
- **AND** the replayed request answers "Done." and the same `write` call
- **THEN** clients receive `session.retry` with `discarded: true` and drop the first half
- **AND** the recorded step holds only "Done." and one `write` call, before and after a restart
- **AND** `write` runs once

#### Scenario: Usage of a discarded attempt
- **WHEN** an attempt without a budget reservation reports 1,200 input tokens and then fails mid-answer with `ProviderInternal` `overloaded_error`
- **THEN** a failed `usage.recorded.1` with purpose `turn` records the 1,200 input tokens and their cost
- **AND** the step's `session.step.ended.1` usage and the context meter hold only the replayed attempt's usage

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
(P1) `fallback_models` SHALL be an ordered list of model refs tried after the Session model when a Turn fails before any output with `RateLimit`, `QuotaExceeded`, `ProviderInternal` or `Transport` after the retry policy is exhausted, or immediately with `Authentication` or `ModelDeniedByPolicyError`. Other kinds (`ContextOverflow`, `InvalidRequest`, `ContentPolicy`) SHALL NOT fall back, and a stream that fails after output SHALL follow the stream retry rules instead. The failed step SHALL be recorded, and the Turn SHALL be re-sent on the next model of the list after the failed one that resolves, skipping the Session's configured model, with reasoning lowering applied. A durable `session.model.fallback.1` event, committed with the failed step, SHALL record `from`, `to`, the error `kind` and `reason`, `started_ms` and `until_ms`. A fallback that fails moves on to the next model; when the list is used up the Turn SHALL fail with the last error, and only then SHALL a usage limit wait be considered. The Session SHALL use the fallback until `until_ms`, which is `fallback.sticky_minutes` (1 to 1440, default 10) after it started, then return to its configured model at the next Turn; the user's next explicit model switch, including to the configured model, SHALL end it at once. The Session view SHALL report the active fallback as `fallback`, and the TUI SHALL announce it and show `fallback: <model>` in the status line while it lasts. `fallback_models` and `fallback` are trust-gated like other routing keys, so an untrusted checkout cannot set them. Agents and workflow tiers MAY set their own `fallback_models`. Fallback SHALL never select a model denied by org policy.

#### Scenario: Provider overloaded
- **WHEN** the primary model returns 529 four times and `fallback_models` is `["anthropic/claude-sonnet", "openai/gpt-6"]`
- **THEN** the Turn is re-sent on `anthropic/claude-sonnet`, the event records the fallback, and the footer shows `fallback: anthropic/claude-sonnet`

#### Scenario: The chain moves on
- **WHEN** the primary fails with `Authentication` and `anthropic/claude-sonnet` then fails with `QuotaExceeded`
- **THEN** the Turn is re-sent on `openai/gpt-6`, two `session.model.fallback.1` events record both hops, and no usage limit wait starts

#### Scenario: Return to the configured model
- **WHEN** `fallback.sticky_minutes` is `5` and the user sends a prompt six minutes after the fallback started
- **THEN** that Turn runs on the Session's configured model and the footer no longer shows the fallback

#### Scenario: No fallback for errors another model would repeat
- **WHEN** a Turn fails with `ContextOverflow` and `fallback_models` is set
- **THEN** no fallback model is tried and the overflow handling applies

### Requirement: ChatGPT subscription provider
(P1) When a ChatGPT login is stored, the catalog SHALL include provider `chatgpt` (name `ChatGPT (subscription)`, base URL `https://chatgpt.com/backend-api/codex`), with credential source `login`.

- **Models:** the models the backend lists for the signed-in account (`GET <base>/models?client_version=1.0.0`, keeping only `visibility: "list"` entries). Each SHALL use the backend's display name and context window, have one reasoning variant per supported effort, and have unknown pricing (`unpriced`). `providers.chatgpt` configuration SHALL apply on top of these models.
- **Model list cache:** the list SHALL be cached in `<data>/chatgpt-models.json`. Login SHALL fetch it. A request SHALL refresh it when it is missing or older than 24 hours. A failed fetch SHALL keep the old list, never fail the request, and not be retried for 24 hours. Logout SHALL delete the cache.
- **Without a login:** the catalog SHALL NOT contain `chatgpt`.
- **Reload on login:** a login or logout completed through the provider login API SHALL rebuild the running server's catalog, so `chatgpt` models are listed and resolvable (or gone) without a restart. Turns already running SHALL keep the model they resolved.
- **Requests:** the provider's adapter SHALL send Responses API requests to `<base>/responses` with:
  - the stored access token as bearer,
  - `chatgpt-account-id` set to the stored account ID,
  - `OpenAI-Beta: responses=experimental` and `originator: cyber`,
  - `store: false`,
  - no `max_output_tokens`,
  - `instructions` always present.
- **Protocol:** the `chatgpt-codex` protocol SHALL NOT be selectable through `api.kind` or models.dev data.

#### Scenario: Provider lists the plan's models
- **WHEN** the backend lists `gpt-6-sol` (`visibility: "list"`) and `gpt-reserve` (`visibility: "hide"`) for the account, and the user signs in with `cyber providers login chatgpt`
- **THEN** `cyber models chatgpt` lists `chatgpt/gpt-6-sol` as available and unpriced, and does not list `chatgpt/gpt-reserve`

#### Scenario: Model list refresh failure
- **WHEN** the cached list is older than 24 hours and the backend answers the model list with an error
- **THEN** the Turn still runs, the old list is kept, and no model list request is sent for the next 24 hours

#### Scenario: Subscription request shape
- **WHEN** a Turn runs on `chatgpt/gpt-6-sol` with a 32000-token output cap
- **THEN** the request goes to `https://chatgpt.com/backend-api/codex/responses` with the bearer token, the `chatgpt-account-id` header and `store: false`, and without `max_output_tokens`

#### Scenario: No login
- **WHEN** no ChatGPT login is stored
- **THEN** `cyber models chatgpt` fails with `Provider not found: chatgpt`
#### Scenario: Login without restart
- **WHEN** a server started without a ChatGPT login completes an attempt through `POST /api/v1/providers/chatgpt/login`
- **THEN** `GET /api/v1/models` lists the plan's `chatgpt/*` models and a new Turn can use them, without restarting the server

### Requirement: GitHub Copilot subscription provider
(P1) When a Copilot login is stored, the catalog SHALL include provider `copilot` (name `GitHub Copilot (subscription)`), with credential source `login`. Its base URL SHALL be the API endpoint the last Copilot token named.

- **Models:**
  - The catalog SHALL list the models `GET <api>/models` returns with `model_picker_enabled: true`, leaving out any whose `policy.state` is `disabled`.
  - Each model SHALL take its context and output limits, vision and tool support from the listed capabilities. It SHALL be unpriced.
  - Each model SHALL use the Responses protocol when its `supported_endpoints` include `/responses`, and Chat Completions otherwise.
  - The list SHALL be cached in `<data>/copilot-models.json`: fetched at login, refreshed when older than 24 hours, and kept unchanged for another 24 hours after a failed refresh.
  - `providers.copilot` configuration SHALL apply on top.
- **Without a login:** the catalog SHALL NOT contain `copilot`.
- **Requests:** every request SHALL carry:
  - the Copilot API token as bearer,
  - `Copilot-Integration-Id`,
  - `Editor-Version: cyber/<version>`,
  - `Openai-Intent: conversation-edits`,
  - `X-Initiator`: `user` for the first model call after a promoted user prompt (an input with `source` `user`, an MCP server prompt the user ran as a slash command, or a `/loop` iteration), and `agent` for every other model call: later calls of the same Turn, calls a tool or hook makes during it (prompt hook checks, `webfetch` summaries, MCP sampling), the first call of a Turn another source opens (Job and monitor handbacks, cross-Session, channel and team messages, workflow results, hook rows, child Sessions' tasks), titles, compaction and recaps.

  Requests with image input SHALL also carry `Copilot-Vision-Request: true`.
- **Protocol:** the Copilot protocol SHALL NOT be selectable through `api.kind` or models.dev data.

#### Scenario: Premium requests counted once per prompt
- **WHEN** a user prompt on `copilot/claude-sonnet-4.5` produces one model call, two tool calls and a second model call
- **THEN** the first request carries `X-Initiator: user` and the second carries `X-Initiator: agent`

#### Scenario: Calls besides the prompt's are the agent's
- **WHEN** a prompt's Turn runs a prompt hook check, a `webfetch` with a prompt on a `copilot` small model and an `agent` delegation to a child Session
- **THEN** the hook check, the summary and the child Session's calls carry `X-Initiator: agent`, and only the prompt's first call carries `user`

#### Scenario: A handback opens an agent Turn
- **WHEN** a background Job's handback (`source: "session"`) is queued and opens a Turn
- **THEN** that Turn's first request carries `X-Initiator: agent`

#### Scenario: Responses-only model
- **WHEN** Copilot lists `gpt-5.6-sol` with `supported_endpoints: ["/responses"]`
- **THEN** Turns on `copilot/gpt-5.6-sol` are sent to `<api>/responses`

#### Scenario: Org-disabled model hidden
- **WHEN** Copilot lists a model with `policy.state: "disabled"`
- **THEN** `cyber models copilot` does not list it

### Requirement: Image and PDF input
(P0) Adapters SHALL send user image and PDF parts in each protocol's native form:

- **`anthropic`:** `image` with a base64 source, and `document` with a base64 `application/pdf` source.
- **`openai_responses` and `chatgpt`:** `input_image` with a data URL, and `input_file` with `filename` and a data-URL `file_data`.
- **`openai_chat`:** `image_url` with a data URL, and `file` with `filename` and a data-URL `file_data`.
- **`copilot`:** its route's form, with `Copilot-Vision-Request: true` on requests that carry an image.

Image input SHALL be allowed only for models whose catalog entry has `capabilities.vision` (from models.dev input modalities, the Copilot model list, or `providers.<id>.models.<id>.capabilities`), and PDF input only with `capabilities.pdf`. A stored attachment reference that reaches an adapter unresolved SHALL fail the request with an invalid-request error instead of being sent or dropped.

#### Scenario: PDF to Anthropic
- **WHEN** a Turn on an Anthropic model with `capabilities.pdf` carries a PDF attachment
- **THEN** the request contains a `document` block with `media_type: "application/pdf"` and the base64 data

#### Scenario: Local model without declared vision
- **WHEN** a configured Ollama model has no `capabilities.vision` and the user attaches a screenshot
- **THEN** the prompt is refused with `UnsupportedModalityError`, whose message names the model

### Requirement: Catalog source integrity
(P0) The downloaded catalog SHALL NOT decide where a first-party provider's key is sent or which variable it reads. For `openai` and `anthropic` the endpoint SHALL be the built-in one (`https://api.openai.com/v1`, `https://api.anthropic.com/v1`) and the key variable `OPENAI_API_KEY` or `ANTHROPIC_API_KEY`, whatever the catalog's `api`, `env` or a model's `provider.api` says. No other catalog provider SHALL read those variables. A catalog endpoint SHALL be used only over HTTPS, or plain HTTP to a loopback server. The user's own `providers` configuration (global, or a trusted project's) MAY still change endpoints and variables.

#### Scenario: Tampered catalog
- **WHEN** a downloaded catalog sets `anthropic.api` to another host and adds a provider whose `env` is `OPENAI_API_KEY`
- **THEN** Anthropic requests still go to `https://api.anthropic.com/v1`, and the added provider has no credential and is never chosen

### Requirement: Refusals and finish reasons
(P1) A model's refusal SHALL reach the user as the answer's text: the OpenAI Responses adapter SHALL treat `response.refusal.delta` and the Chat Completions adapter SHALL treat `delta.refusal` as text deltas. Each finished assistant message SHALL carry `finish`, the step's finish reason (`stop`, `tool_calls`, `length`, `content_filter` or another provider value), in the messages API; it SHALL be null while the step runs and when it failed.

#### Scenario: Streamed refusal
- **WHEN** a Responses stream sends `response.refusal.delta` with "I can't help with that." and ends incomplete with reason `content_filter`
- **THEN** the assistant message's text is "I can't help with that." and its `finish` is `content_filter`
