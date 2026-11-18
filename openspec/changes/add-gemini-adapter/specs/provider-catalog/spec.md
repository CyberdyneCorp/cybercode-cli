## ADDED Requirements

### Requirement: Gemini API adapter
(P1) The system SHALL ship a native adapter for the Gemini API protocol `gemini` (accepted in `api.type` as `gemini`, `google` or `google-genai`), and SHALL map the models.dev package `@ai-sdk/google` to it; `@ai-sdk/google-vertex` SHALL stay `adapter_unsupported`. The adapter SHALL:
- stream `POST <base>/models/<api_id>:streamGenerateContent?alt=sse`, sending the full history on every request (no server-side state);
- send the API key only in the `x-goog-api-key` header, never in the URL;
- default the base URL to `https://generativelanguage.googleapis.com/v1beta`, remove a trailing `/`, and append `/v1beta` when the configured URL has no path;
- refuse a base URL that is not `https://` unless its host is a loopback address;
- emit only the provider-neutral `LlmEvent` stream, with Gemini thought summaries (`thought: true` parts) as `reasoning.delta`;
- send `systemInstruction` from the system prompt, `inlineData` for images and PDFs, and `functionCall` / `functionResponse` parts for tool calls and results, carrying the call `id` when the model issued one.

The bundled catalog snapshot SHALL include the `google` provider.

#### Scenario: Key alone enables Gemini
- **WHEN** `GEMINI_API_KEY` is set and no `providers.google` config exists
- **THEN** `cyber models google` lists Gemini models as available, and a Turn on `google/gemini-4-carbon` posts to `https://generativelanguage.googleapis.com/v1beta/models/gemini-4-carbon:streamGenerateContent?alt=sse` with the key in `x-goog-api-key` and not in the URL

#### Scenario: Base URL with a version suffix
- **WHEN** config sets `providers.google.api.url` to `https://generativelanguage.googleapis.com/v1beta/`
- **THEN** requests go to `https://generativelanguage.googleapis.com/v1beta/models/<model>:streamGenerateContent?alt=sse`

#### Scenario: Plain HTTP to a remote host
- **WHEN** a provider with `api.type: "gemini"` is configured with `api.url: "http://gemini.corp.example"`
- **THEN** its models are unavailable with an error naming the URL, and no request is sent

### Requirement: Gemini thought signatures
(P1) The Gemini adapter SHALL keep every `thoughtSignature` exactly as received (`OpaqueKind::Signature`, never decoded or altered) and return it on the same part it came on:
- a signature on a `functionCall` part SHALL be stored on that tool call and replayed on the same `functionCall`;
- a signature on any other part, including a part with empty text at the end of a stream, SHALL close a reasoning item and be replayed as the last part of that model content;
- the decoder SHALL read the stream until `finishReason` and the stream's end before finishing, so a late signature is not lost.

For a step with parallel calls, only the calls that arrived with a signature SHALL carry one: the adapter SHALL NOT copy, move or invent signatures. A replayed step SHALL be one model content holding its calls in their original order, followed by one user content holding all their `functionResponse`s in the same order (`FC1(+sig), FC2, FR1, FR2`). A signed part SHALL NOT be merged with another part.

The `gemini` protocol's profile SHALL default to `reasoning_carrier: gemini_signature` and `reasoning_replay: always`. Signatures SHALL be sent only to the provider and model that produced them. In the current turn (the messages after the latest user message with non-tool content), tool calls without a signature from the Turn's model, and their results, SHALL be lowered to text parts before the request is sent; the adapter SHALL NOT send a placeholder signature unless the model's profile sets `signature_bypass: true`.

#### Scenario: Parallel calls keep one signature
- **WHEN** a Gemini 4 step returns `functionCall` `read` with signature `S1` and `functionCall` `grep` without one, and both tools complete
- **THEN** the next request's history holds a model content `[read + S1, grep]` followed by a user content `[functionResponse read, functionResponse grep]`, and `grep` carries no signature

#### Scenario: Signature in an empty final part
- **WHEN** a streamed text answer ends with a chunk whose part has `text: ""`, `thoughtSignature: "S2"` and `finishReason: "STOP"`
- **THEN** a reasoning item with signature `S2` is recorded, and the next request sends `S2` on the last part of that model content

#### Scenario: Model switch inside a tool loop
- **WHEN** the current turn holds an `anthropic/claude-sonnet` tool call and its result, and the user switches the Session to `google/gemini-4-carbon`
- **THEN** that call and result are sent to Gemini as text parts, no `functionCall` without a signature is sent for the current turn, and the request does not fail with a missing-signature 400

### Requirement: Gemini thinking configuration
(P1) The Gemini adapter SHALL send `generationConfig.thinkingConfig` with exactly one of `thinkingLevel` or `thinkingBudget`, never both:
- models whose API ID starts with `gemini-2.5` SHALL receive `thinkingBudget`; later models SHALL receive `thinkingLevel`; the profile field `thinking_param` (`level` or `budget`) SHALL override this;
- an effort SHALL become `thinkingLevel`, and a budget sent to a level model SHALL be converted to the nearest level with one `info` log line;
- reasoning off SHALL send `thinkingBudget: 0` only to models that can turn thinking off; for others (Gemini 4, Gemini 3 Pro, Gemini 2.5 Pro) it SHALL use the lowest supported level or budget, except that an explicit user variant SHALL fail with `VariantUnavailableError`;
- `includeThoughts: true` SHALL be sent whenever thinking is on.

`maxOutputTokens` SHALL be the request's value, else the model's catalog output limit, else 65,535, and SHALL never exceed 65,535. Temperature SHALL be sent only when an agent, variant or config sets one.

#### Scenario: Level for Gemini 4
- **WHEN** a Turn on `google/gemini-4-hydrogen#high` is sent
- **THEN** `thinkingConfig` is `{ "thinkingLevel": "high", "includeThoughts": true }` and has no `thinkingBudget`

#### Scenario: Budget for Gemini 2.5
- **WHEN** a Turn on `google/gemini-2.5-flash` uses a variant with a 4,096-token budget
- **THEN** `thinkingConfig` is `{ "thinkingBudget": 4096, "includeThoughts": true }` and has no `thinkingLevel`

#### Scenario: Default output limit
- **WHEN** a Gemini model's catalog record has no output limit and the request sets none
- **THEN** the request sends `maxOutputTokens: 65535`

### Requirement: Gemini error and quota classification
(P1) The Gemini adapter SHALL classify failures from the HTTP status and the `google.rpc` details of the error body (`ErrorInfo.reason`, `QuotaFailure.violations[]` quota ids and metrics, `RetryInfo.retryDelay`):
- 402, or a prepaid credit balance message → `QuotaExceeded` with "Gemini prepaid credit balance is exhausted";
- 429 whose quota is per day or per week, or that names a spend cap → `QuotaExceeded` with a message naming the exhausted quota or the spend cap;
- any other 429 (per-minute request or token limits, capacity, or no details) → `RateLimit`, with `retry_after` taken from `RetryInfo`;
- 500, 503 and 504 → `ProviderInternal`;
- 400 naming a missing thought signature → `InvalidRequest` "Gemini rejected the history: missing thought signature";
- 400 about the input token count → `ContextOverflow`;
- 400 `FAILED_PRECONDITION`, 401 and 403 → `Authentication`;
- other 4xx → `InvalidRequest`.

Retry policy's cap on a server-advised delay SHALL be per provider (`providers.<id>.retry.max_wait_ms`), defaulting to 60 seconds and to 240 seconds for the `gemini` protocol. When an advised delay is longer than the cap, the step SHALL fail at once with that error instead of waiting a shorter time.

#### Scenario: Per-minute limit waits and recovers
- **WHEN** a request answers 429 with a `QuotaFailure` whose quota id ends in `PerMinutePerProjectPerModel` and a `RetryInfo` of `37s`, and the next attempt succeeds
- **THEN** the error is `RateLimit`, one `session.retry` announces a 37-second delay, and the Turn completes

#### Scenario: Daily quota stops at once
- **WHEN** a request answers 429 with a `QuotaFailure` whose quota id contains `PerDay`
- **THEN** the error is `QuotaExceeded`, no retry is attempted, `fallback_models` is tried next when configured, and otherwise the Turn fails with a message saying the daily quota is exhausted

#### Scenario: Prepaid balance exhausted
- **WHEN** a request answers 402 `RESOURCE_EXHAUSTED`
- **THEN** the error is `QuotaExceeded`, no retry is attempted, and the message tells the user the prepaid credit balance is exhausted

#### Scenario: Advised delay beyond the cap
- **WHEN** a 429 classified as `RateLimit` advises a 10-minute delay and the provider's cap is 240 seconds
- **THEN** no retry is attempted and the step fails with `RateLimit` naming the advised delay

### Requirement: Gemini finish reasons and safety blocks
(P1) The Gemini adapter SHALL map finish signals as follows, and SHALL never retry or fall back on a safety block:
- `STOP` → `stop`, or `tool_calls` when the step produced calls;
- `MAX_TOKENS` → `length`;
- `SAFETY`, `PROHIBITED_CONTENT`, `BLOCKLIST`, `SPII`, `IMAGE_SAFETY` and `RECITATION` → `content_filter` when output was already streamed (the output is kept), otherwise a `ContentPolicy` error naming the reason;
- `promptFeedback.blockReason` → a `ContentPolicy` error naming the reason;
- `MALFORMED_FUNCTION_CALL` → `ProviderInternal` "Gemini produced a malformed function call", retried under Retry policy;
- `UNEXPECTED_TOOL_CALL` → `InvalidRequest`;
- `MISSING_THOUGHT_SIGNATURE` → `InvalidRequest`, as for the 400 above;
- `TOO_MANY_TOOL_CALLS` → finish `too_many_tool_calls`, keeping the calls received;
- any other value → finish with that value in lower case.

#### Scenario: Prompt blocked
- **WHEN** a response has no candidates and `promptFeedback.blockReason` is `PROHIBITED_CONTENT`
- **THEN** the step fails with `ContentPolicy` naming `PROHIBITED_CONTENT`, and no retry or fallback is attempted

#### Scenario: Malformed function call resampled
- **WHEN** the first attempt ends with `finishReason: MALFORMED_FUNCTION_CALL` and no output, and the retry returns a valid call
- **THEN** the valid call is dispatched once and the step records only the retried attempt

### Requirement: Gemini usage and implicit caching
(P1) The Gemini adapter SHALL report the last `usageMetadata` of a response as usage with `cache_read` = `cachedContentTokenCount`, `input` = `promptTokenCount` + `toolUsePromptTokenCount` − `cachedContentTokenCount`, `reasoning` = `thoughtsTokenCount`, `output` = `candidatesTokenCount` and `cache_write` = 0. To keep implicit caching effective, the system instruction SHALL hold only the Context Epoch's baseline, tool declarations SHALL be sent in the registry's deterministic order with a fixed key order, and replayed history SHALL be byte-identical between requests of a Session. Explicit `cachedContents` SHALL NOT be created.

#### Scenario: Cached tokens reported
- **WHEN** the final chunk reports `promptTokenCount: 120000`, `cachedContentTokenCount: 100000`, `thoughtsTokenCount: 800` and `candidatesTokenCount: 300`
- **THEN** usage is `input 20000`, `cache_read 100000`, `reasoning 800`, `output 300`, and cost prices the cache read at the model's cache-read rate

#### Scenario: Stable prefix
- **WHEN** two consecutive requests of a Session differ only by a new user message
- **THEN** the bytes of `systemInstruction`, `tools` and every earlier `contents` entry are identical in both requests

### Requirement: Gemini tool declarations
(P1) The Gemini adapter SHALL declare each tool as a `functionDeclaration` with `name`, `description` and `parametersJsonSchema`, removing `$schema`, `$id` and `examples` keys. A 400 `INVALID_ARGUMENT` about a function declaration or schema SHALL be an `InvalidRequest` error that names the tool when the provider's message names it and suggests deferred tool loading or disabling large MCP servers.

#### Scenario: Oversized MCP schema
- **WHEN** an MCP tool's schema makes Gemini answer 400 `INVALID_ARGUMENT` naming `big_tool`
- **THEN** the step fails with `InvalidRequest` naming `big_tool`, no retry is attempted, and the message suggests MCP tool search or disabling that server

### Requirement: Gemini stream resilience
(P1) The stream idle timeout SHALL default to 600 seconds for the `gemini` protocol; `providers.<id>.timeout.idle_ms` SHALL still override it. When a request fails before any response header because a pooled connection was closed or reset, the adapter SHALL send it once more at once on a new connection; that resend SHALL NOT count as a Retry policy attempt or publish `session.retry`, and a second such failure SHALL be an ordinary `Transport` failure.

#### Scenario: Long silent thinking
- **WHEN** a Gemini stream sends its first chunk and then nothing for 8 minutes while the model thinks, then completes
- **THEN** the step completes without an idle-timeout failure

#### Scenario: Stale HTTP/2 connection
- **WHEN** the first send of a request fails with a connection reset before any header, and the resend succeeds
- **THEN** the Turn completes with no `session.retry` event and four Retry policy attempts still available
