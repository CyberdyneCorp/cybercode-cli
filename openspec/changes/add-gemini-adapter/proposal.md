## Why

`provider-catalog` → Additional native provider adapters plans a native Google Gemini adapter for P1. Today Gemini models reach Cyber Code only through the OpenAI-compatible endpoint (`improve-model-protocol-fidelity` added the `extra_content.google.thought_signature` carrier for it), through a Copilot plan, or through a gateway. `GEMINI_API_KEY` alone enables nothing: models.dev lists `google` with `@ai-sdk/google`, which maps to no adapter, so its models are `adapter_unsupported` and the bundled snapshot drops the provider.

The compatible endpoint is a lossy translation layer for an agent loop:

- **Thought signatures.** Gemini 3 and 4 reject a request whose current-turn `functionCall` lost its signature (HTTP 400). The native protocol defines exactly where signatures live (the first `functionCall` part of a step, or the last part of a text answer, possibly an empty-text part at the end of a stream), and the compatible layer only partly exposes them.
- **Quota errors.** Gemini answers every quota problem with 429 `RESOURCE_EXHAUSTED` and a body that says "quota". Our `classify` maps any 429 mentioning "quota" to `QuotaExceeded`, which is never retried, so a per-minute limit that clears in 30 seconds ends the Turn. The native error carries `google.rpc.QuotaFailure` and `RetryInfo` details that tell a per-minute limit from a daily one, and 402 signals an empty prepaid balance.
- **Thinking.** Gemini 3+/4 take `thinkingLevel`, 2.5 takes `thinkingBudget`, sending both is a 400, and thinking counts toward `maxOutputTokens`. Long high-effort answers can stay silent for more than 5 minutes while thinking, which trips our 300 s stream idle timeout.
- **Safety and finish reasons.** `SAFETY`, `PROHIBITED_CONTENT`, `promptFeedback.blockReason`, `MALFORMED_FUNCTION_CALL`, `MISSING_THOUGHT_SIGNATURE` and others need typed handling: some are terminal, some are worth one resample.
- **Usage.** `cachedContentTokenCount` (implicit caching) and `thoughtsTokenCount` must be reported as cache-read and reasoning tokens, or cost and the context meter are wrong.

## What Changes

- **New protocol `gemini`** (`provider-catalog`, added "Gemini API adapter"). `ApiKind::Gemini` (`gemini`, aliases `google` and `google-genai`), mapped from the models.dev package `@ai-sdk/google`. The adapter streams `POST {base}/models/{model}:streamGenerateContent?alt=sse` with the key in `x-goog-api-key`, replays the full history statelessly (as every other adapter does), and decodes through the shared `decode_sse` with a new `Decoder`. It ships behind the experimental feature `gemini` (off by default) until the live coding baseline is published.
- **Thought signatures** (`provider-catalog`, added "Gemini thought signatures"). Signatures are stored as `OpaqueKind::Signature` on the call (`Content::ToolCall.opaque`) or on a reasoning item, returned byte-exact in the same part, never merged across parts, with the parallel-call order `FC1(+sig), FC2, FR1, FR2`. History from another model is sent as text. The documented bypass placeholder is used only when a model's profile sets `signature_bypass: true`, an option that is off by default and documented as discouraged.
- **Thinking configuration** (`provider-catalog`, added "Gemini thinking configuration"). Effort maps to `thinkingLevel` on Gemini 3+/4 and a budget maps to `thinkingBudget` on 2.5, never both; thinking stays on where it cannot be turned off; `maxOutputTokens` defaults to the model limit, at most 65,535.
- **Errors and quota** (`provider-catalog`, added "Gemini error and quota classification"). Parse `google.rpc` `ErrorInfo`, `QuotaFailure` and `RetryInfo`: per-minute and capacity limits wait the server's delay (up to a per-provider cap of 240 s) and retry; daily or weekly quotas, spend caps and 402 prepaid balance stop at once with a clear message. A per-provider `retry.max_wait_ms` replaces the fixed 60 s cap for this protocol. When the server asks for longer than the cap, an interactive Session fails at once, and an unattended run (`exec`, workflows, background Jobs, Goals, loops) waits the full delay within `usage_limit.max_wait` and its wall deadlines.
- **Finish reasons and safety** (`provider-catalog`, added "Gemini finish reasons and safety blocks").
- **Usage and implicit caching** (`provider-catalog`, added "Gemini usage and implicit caching").
- **Tool declarations** (`provider-catalog`, added "Gemini tool declarations"): `parametersJsonSchema`, deterministic order, a clear error when a schema is too large, with MCP tool search as the mitigation; no automatic schema shrinking.
- **Stream resilience** (`provider-catalog`, added "Gemini stream resilience"): a 600 s idle default for this protocol, and one immediate retry on a fresh connection when a pooled HTTP/2 connection turns out stale.
- **Credentials** (`provider-credentials`, added "Gemini API key" and "Google subscription login refused"). Config or a stored key, then `GEMINI_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY` and `GOOGLE_API_KEY`, in that order; the `google` provider's endpoint is pinned like `openai`/`anthropic`; `/login google` stays refused, and the reason is recorded.
- **Default model** (`provider-catalog`, added "Gemini default model choice"). With only a Google key, the default is the newest available Gemini Pro-tier model, chosen by the same newest-`released_at` rule as for other providers.
- **Profile defaults.** The `gemini` protocol defaults to `reasoning_carrier: gemini_signature` and `reasoning_replay: always`, so the prefix stays byte-stable for implicit caching; the `gemini-3`/`gemini-4` name rules for the compatible path stay as they are.

## Non-goals (follow-up changes)

- **Vertex AI and Application Default Credentials.** Same wire format at a different host with OAuth access tokens; design note only (design.md D11). It belongs with the planned `google-vertex` adapter.
- **Consumer subscriptions.** Google AI Pro/Ultra, Gemini Code Assist for individuals and Antigravity OAuth stay refused (see "Google subscription login refused").
- **Explicit context caching** (`cachedContents`). It is not offered on Gemini 4; implicit caching covers the MVP.
- **The Interactions API.** Server-side state does not fit our full-history replay and event-sourced Sessions. Recorded as a future option (design.md D12).
- **Automatic tool-schema shrinking.** Not done; MCP tool search is the mitigation. A possible follow-up.
- **Planner/executor routing** like the Antigravity Orchestrator. If wanted, it is a follow-up on top of model roles and the fallback chain, not part of an adapter.
- **Reserved budgets.** `prepare_reserved` keeps refusing for this protocol, as for every adapter except OpenAI Responses.
- Gemini Live, image/audio/video output, code execution, Google Search grounding and URL context tools.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

All requirements are ADDED. No existing requirement is modified, so this change cannot conflict with open changes.

- `provider-catalog`: "Gemini API adapter", "Gemini thought signatures", "Gemini thinking configuration", "Gemini error and quota classification", "Gemini finish reasons and safety blocks", "Gemini usage and implicit caching", "Gemini tool declarations", "Gemini default model choice" and "Gemini stream resilience". They implement the Gemini part of "Additional native provider adapters".
- `provider-credentials`: "Gemini API key" and "Google subscription login refused".

## Impact

- **Code:**
  - `crates/cyber-llm/src/adapters/gemini.rs` (new; request builder, `Decoder`, error parser), `adapters/mod.rs` (`ApiKind::Gemini`: `parse`, `from_npm`, `default_url`, `as_str`, `adapter()`).
  - `error.rs`: a Gemini error parser feeding the existing `ErrorKind`s; `retry.rs`: per-provider `retry_after_cap`.
  - `catalog/profile.rs` (protocol defaults), `catalog/credentials.rs` and the catalog source integrity check (pinned `google` endpoint and variables), `catalog/family.rs` unchanged.
  - `scripts/refresh_models_snapshot.py` (`@ai-sdk/google` in `SUPPORTED_NPM`) and a refreshed `snapshot/models.json.gz`.
  - The feature registry (`gemini`, experimental), `catalog/roles.rs` (`default_model` Pro-tier filter) and the runtime's retry path (unattended full wait).
  - `crates/cyber-tui/src/login.rs`: the refusal text no longer says Gemini awaits an adapter.
- **Data:** none new. Signatures use the existing optional `opaque` fields of `session.tool.called.1` and `session.reasoning.ended.1`.
- **API, SDK and CLI:** `api.type: "gemini"` becomes valid in `providers`; `cyber models` lists `google/*`. No new route or event.
- **Licensing:** clean-room. Behaviour is taken from Google's public documentation and the Antigravity CLI changelog; the Gemini CLI quota classifier was read only as an idea reference. No code or text is copied.
- **Docs:** `docs/local-core-guide.md` (a "Gemini API" section beside the subscription sections), `docs/features.md`, README provider table and Known gaps, `ROADMAP.md`.

## Sources

- Google AI Studio documentation (generateContent variants), read 2026-11-18: [Thought signatures](https://aistudio.google.com/docs/thought-signatures.md?generate-content=true), [Thinking](https://aistudio.google.com/docs/thinking.md?generate-content=true), [Function calling](https://aistudio.google.com/docs/function-calling.md?generate-content=true), [Caching](https://aistudio.google.com/docs/caching.md?generate-content=true), [API errors](https://aistudio.google.com/docs/api-errors.md?generate-content=true), [Rate limits](https://aistudio.google.com/docs/rate-limits.md), [Terms](https://aistudio.google.com/docs/terms.md), [streamGenerateContent](https://aistudio.google.com/api-reference/generate-content/stream-generate-content.md), [Interactions overview](https://aistudio.google.com/docs/interactions-overview.md).
- [Antigravity CLI CHANGELOG](https://github.com/google-antigravity/antigravity-cli/blob/main/CHANGELOG.md): 1.5.10 (waits up to 4 minutes on advised retry delays; spend-cap, prepaid-balance and weekly quota errors stop the turn; 10-minute silence tolerance while thinking), 1.5.8 (retry on a fresh connection after a stale or force-closed HTTP/2 connection; signature error after switching from Gemini to another model), 1.5.3 (`http2: timeout awaiting response headers` retried), 1.5.0 (429/5xx honour `Retry-After`), 1.4.7 (very large signatures), 1.2.2 (signatures on text and thought parts for `GEMINI_API_KEY` sessions).
- Antigravity terms, clause 6, and the Gemini CLI terms on third-party use of their OAuth; the 2026-06-18 end of consumer-tier service through Gemini CLI; account suspensions reported in 2026.
- Gemini CLI quota error classification: idea reference only.
