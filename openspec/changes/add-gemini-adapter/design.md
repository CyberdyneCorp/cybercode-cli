## Context

The research behind this change is Google's AI Studio documentation for the `generateContent` API (thought signatures, thinking, function calling, caching, API errors, rate limits, terms; read 2026-11-18), the Antigravity CLI changelog (1.2.2 to 1.5.11) and, as an idea reference only, the quota error classifier in Gemini CLI. See proposal.md → Sources. The documentation now leads with the Interactions API; every rule below is from the `generateContent` variants of those pages.

Where we stand (`main` at `386a49cc`):

- `crates/cyber-llm/src/adapters/mod.rs`: `trait Adapter { stream; prepare_reserved (default refuses); with_http_client (default None) }`, `ApiKind` with `parse`, `from_npm`, `default_url`, `as_str` and `adapter()`, `Endpoint` with `idle_timeout` (default 300 s), `decode_sse` driving a `Decoder { on_event, on_end }`, and `ToolCalls` with `start`, `set_opaque`, `append`, `finish`.
- `types.rs`: `Content::ToolCall.opaque` (documented for Gemini thought signatures), `Content::Reasoning { text, opaque }`, `LlmEvent::ReasoningItem { text, opaque }`, `ToolCall.opaque`, `OpaqueKind::Signature`, `Opaque::is_from(provider, model)`, `Reasoning::{Off, Effort, BudgetTokens}`, `Usage { input, output, reasoning, cache_read, cache_write }` (input excludes cache reads, output excludes reasoning, as `openai_responses.rs` computes it).
- `error.rs`: `ErrorKind` (`Authentication`, `RateLimit`, `QuotaExceeded`, `ContextOverflow`, `ContentPolicy`, `InvalidRequest`, `ProviderInternal`, `Transport`); `classify` maps any 429 whose body contains "quota" to `QuotaExceeded`. Every Gemini 429 says "quota", so a per-minute limit is not retried today.
- `retry.rs`: `RetryPolicy { max_retries: 4, retry_after_cap: 60 s, … }`; an advised delay above the cap is shortened to the cap, so the retry arrives too early and burns an attempt.
- `catalog/profile.rs`: `ReasoningCarrier::GeminiSignature`; rule `by_name("gemini-3", chat_only)`; PR #144 (open) adds `gemini-4` for the compatible path. `defaults()` sets native carriers only for Responses and Anthropic.
- `catalog/family.rs` maps `gemini*` to `ModelFamily::Gemini`, which selects `prompts/base/gemini.md`.
- `scripts/refresh_models_snapshot.py` keeps only providers whose npm package maps to an adapter, so `google` is absent from the bundled snapshot.
- `crates/cyber-tui/src/login.rs`: `/login google|gemini` refuses with "Google subscription login is not supported, and Gemini models await a native Gemini adapter".

## Goals / Non-Goals

**Goals:**
- A Gemini 3/4 tool loop never fails on a signature the adapter lost, reordered or merged.
- A per-minute limit waits and recovers; a daily quota, spend cap or empty balance stops at once and says why.
- Long thinking does not trip the idle timeout; one stale pooled connection does not cost a retry.
- Usage and cost are exact, including implicit cache reads and thinking tokens.
- Nothing provider-specific crosses the adapter boundary.

**Non-goals:** see proposal.md.

## Decisions

### D1. Protocol, endpoint and authentication
- `ApiKind::Gemini`, `as_str` `gemini`; `parse` accepts `gemini`, `google` and `google-genai`; `from_npm("@ai-sdk/google")`. `@ai-sdk/google-vertex` stays unmapped (`adapter_unsupported`).
- `default_url` is `https://generativelanguage.googleapis.com/v1beta`. A configured base URL is normalized: a trailing `/` is removed, and `/v1beta` is appended when the path is empty, so both `https://generativelanguage.googleapis.com` and `…/v1beta/` work. Any other path (a proxy prefix) is kept as written. A non-loopback URL must be `https://`; plain `http://` is accepted only for loopback (tests, local proxies).
- Key variables, first set wins after config and stored keys: `GEMINI_API_KEY`, `GOOGLE_GENERATIVE_AI_API_KEY`, `GOOGLE_API_KEY`. `GOOGLE_API_KEY` is generic, so it comes last, and the catalog pinning keeps any other catalog provider from reading it.
- Request: `POST {base}/models/{api_id}:streamGenerateContent?alt=sse`. The key goes in `x-goog-api-key`, never in the query string, so it cannot appear in a logged URL. No redirects are followed (existing P0 rule).
- `generateContent` is stateless, which matches our full-history replay; nothing is stored server side.
- **Experimental gate.** The feature registry entry `gemini` (maturity `experimental`, off by default) gates the protocol until the live coding baseline from "Additional native provider adapters" is published. While it is off, `gemini`-protocol models are unavailable with the existing reason `experimental` and the hint `enable with features.gemini`. The change that records the baseline moves the entry to `stable`, which turns it on by default.
- `with_http_client` is implemented, so interruption can verify transport termination as for the other adapters. `prepare_reserved` keeps the default refusal.

### D2. Request shape
```
{
  "systemInstruction": { "parts": [{ "text": <system> }] },
  "contents": [ { "role": "user" | "model", "parts": [...] } ],
  "tools": [{ "functionDeclarations": [{ "name", "description", "parametersJsonSchema" }] }],
  "toolConfig": { "functionCallingConfig": { "mode": "AUTO" } },
  "generationConfig": {
    "maxOutputTokens": <n>,
    "temperature"?: <only when an agent or config sets one>,
    "thinkingConfig": { "thinkingLevel" | "thinkingBudget", "includeThoughts": true }
  }
}
```
- Lowering: `Text` → `{text}`; `Image`/`Document` → `{inlineData: {mimeType, data}}`; `ToolCall` → `{functionCall: {id?, name, args}}` plus `thoughtSignature` when it has one; `ToolResult` → `{functionResponse: {id?, name, response: {output} | {error}}}`; `Reasoning` → see D3.
- Consecutive messages of the same role are kept as separate `contents` entries only where the order rules in D3 need it; otherwise one entry per message.
- `id` on `functionResponse` is sent when the call had one (Gemini 3+ issues ids). For models that issue none, the decoder assigns `gm_<step>_<n>` locally, and those ids are not sent; responses are then matched by name and position.
- Temperature is not sent unless set: Google recommends the default for Gemini 3+.

### D3. Thought signatures
- **Capture.** A `thoughtSignature` on a `functionCall` part is stored with `ToolCalls::set_opaque` as `Opaque { kind: Signature, data }` on that call. A signature on any other part (text, empty text, or a `thought: true` summary part) closes a reasoning item: `LlmEvent::ReasoningItem { text: <summary text so far>, opaque: Signature }`. A signature may arrive in a part whose text is empty, in the last chunk; the decoder reads until `finishReason` and the end of the stream before it finishes.
- **Data is exact.** `data` is the base64 string as received; it is never decoded, trimmed or re-encoded. Oversized values follow the existing 1 MiB rule (dropped, replayed as text, one warning).
- **Replay.** For a message whose opaque state `is_from(provider, model)`:
  - each call goes back as `functionCall` with its own `thoughtSignature`, or none when it had none. Only the first call of a parallel step has one; the adapter never copies it to the others and never invents one;
  - one model content holds the step's calls in their original order, and the next user content holds all their `functionResponse`s in the same order: `FC1(+sig), FC2, FR1, FR2`. Interleaving is a 400;
  - a reasoning item's signature goes back on the part it came on: the last part of the model content, as `{text, thoughtSignature}` (empty text when it arrived on an empty part);
  - a signed part is never merged with an unsigned one, and two text parts are never concatenated when either is signed.
- **Profile.** The `gemini` protocol defaults to `reasoning_carrier: gemini_signature`, `reasoning_replay: always`. Google only validates the current turn, but returning every signature keeps the history bytes identical from request to request, which implicit caching needs (D6). `tool_loop` would drop older signatures after each user prompt and change the prefix.
- **Cross-model history.** Under `session-runtime` → Reasoning lowering, calls from another model carry no Gemini signature. In earlier turns that is accepted. In the current turn (from the latest user message with text content; a `functionResponse` does not start a turn) an unsigned `functionCall` would be a 400 on Gemini 3/4, so the adapter lowers that turn's foreign calls and their responses to text parts (`[tool call <name> <json args>]`, `[tool result <name>] <output>`) before the first native signed call. The documented bypass values (`skip_thought_signature_validator`, `context_engineering_is_the_way_to_go`) are not used by default: Google discourages them and they hide our own bugs.
- **`signature_bypass`.** A per-model profile option (`providers.<p>.models.<m>.profile.signature_bypass`), `false` by default and documented as discouraged. When `true`, those foreign current-turn calls are sent as `functionCall` parts with `thoughtSignature: "skip_thought_signature_validator"` instead of text, and one `warn` line per Session names the model and the option. It exists for users whose tool loops depend on structured calls surviving a model switch.
- Gemini 2.5 does not require signatures; the same code path runs, and nothing is lowered for 2.5 because a missing signature is accepted there.

### D4. Thinking
- The model's generation is read from its API ID: `gemini-2.5*` uses `thinkingBudget`; `gemini-3*`, `gemini-4*` and later use `thinkingLevel`. A profile field `thinking_param: level | budget` overrides it.
- `Reasoning::Effort(e)` → `thinkingLevel: e` (levels per model come from the catalog variants; `effort_clamp` applies as today). `Reasoning::BudgetTokens(n)` → `thinkingBudget: n` on 2.5; on 3+/4 it is converted to the nearest level, with one `info` line, because sending both fields is a 400 and Gemini 4 deprecates the budget.
- `Reasoning::Off` → `thinkingBudget: 0` on 2.5 Flash and Flash-Lite; for models that cannot turn thinking off (Gemini 4, 2.5 Pro, Gemini 3 Pro) it becomes the lowest level or minimum budget, and an explicit user variant fails with `VariantUnavailableError` as today.
- `includeThoughts: true` is always sent where thinking is on, so summaries stream as `reasoning.delta`.
- `maxOutputTokens`: the request's own value, else the catalog output limit, else 65,535, and never above 65,535 (Gemini 4 streaming caps output at 65,536, thinking included). Thinking counts toward it, so a lower agent setting can end an answer with `MAX_TOKENS` before any text.

### D5. Errors and quota
A non-2xx body is `{error: {code, message, status, details[]}}`. `details` entries are typed by `@type`:
- `type.googleapis.com/google.rpc.ErrorInfo` → `reason`, `domain`, `metadata`;
- `type.googleapis.com/google.rpc.QuotaFailure` → `violations[].quotaMetric`, `quotaId`, `quotaDimensions`, `quotaValue`;
- `type.googleapis.com/google.rpc.RetryInfo` → `retryDelay` (`"37s"`, `"1.5s"`).

Classification, first match wins:

| Signal | Kind | Retry |
|---|---|---|
| 402, or a message about prepaid credits / balance | `QuotaExceeded`, "Gemini prepaid credit balance is exhausted; add credits in AI Studio" | never |
| 429 with a quota id or metric that is per day or per week (`PerDay`, `PerWeek`, `RPD`) | `QuotaExceeded`, "Gemini daily quota exhausted for <model>" | never |
| 429 mentioning a spend cap / spending limit | `QuotaExceeded`, "Gemini project spend cap reached" | never |
| 429 otherwise (per minute, tokens per minute, capacity, or no details) | `RateLimit`, `retry_after` from `RetryInfo` | yes |
| 503 `UNAVAILABLE`, 500 `INTERNAL`, 504 `DEADLINE_EXCEEDED` | `ProviderInternal` | yes |
| 400 with "thought signature" / `thought_signature` | `InvalidRequest`, "Gemini rejected the history: missing thought signature" | never |
| 400 `INVALID_ARGUMENT` about the token count / input too long | `ContextOverflow` | never |
| 400 `INVALID_ARGUMENT` about a function declaration or schema | `InvalidRequest`, naming the tool when the message names it | never |
| 400 `FAILED_PRECONDITION` (free tier not available in the region) | `Authentication`, "Gemini API not available for this project or region" | never |
| 401, 403 | `Authentication` | never |

`QuotaExceeded` is already a reason the fallback chain moves on for. The messages above are what the TUI and `exec` show.

**Retry wait cap.** `RetryPolicy.retry_after_cap` becomes per provider: `providers.<p>.retry.max_wait_ms`, default 60,000, and 240,000 for the `gemini` protocol. When the advised delay is longer than the cap, the step never waits a shortened time (that retries before the window opens and spends an attempt). What happens instead depends on whether anyone is waiting for the answer:
- **Interactive Session:** fail at once with the `RateLimit` error naming the advised delay, so the fallback chain applies and the user can decide.
- **Unattended run:** wait the full advised delay, then retry. The wait is one Retry policy attempt, announced by `session.retry` with the full delay. It is bounded by `usage_limit.max_wait` (default 6 h) and by every wall deadline of the Session (Goal, loop iteration, Workflow Run); a wait that would cross either fails at once instead.

**Unattended** reuses the existing definition so there is one notion in the product: `usage_limit.mode` is `always`, or it is `unattended` (default) and the Session is unattended under `session-runtime` → Usage limit wait (non-interactive server, `unattended: true` as `cyber exec` sets it, Workflow agent, active Goal, loop iteration in flight, non-lead team member, or a subagent of an unattended parent). In addition, a Session running as a background Job (`kind` `subagent` or `workflow`) counts as unattended for this rule, since nobody watches it directly and its parent is not blocked. `usage_limit.mode: never` turns the full wait off everywhere. The rule lives in `retry.rs`, which receives an `unattended` flag and the deadline from the runtime; the adapter only reports `retry_after`.

### D6. Usage and implicit caching
`usageMetadata` arrives on the last chunk (and sometimes on earlier ones; the last one wins):
- `cache_read = cachedContentTokenCount`;
- `input = promptTokenCount + toolUsePromptTokenCount - cachedContentTokenCount` (`promptTokenCount` includes cached tokens);
- `reasoning = thoughtsTokenCount`;
- `output = candidatesTokenCount` (it excludes thoughts);
- `cache_write = 0` (implicit caching has no write charge).

Implicit caching hits when a request shares a long prefix with a recent one. The adapter keeps that prefix stable: the system instruction is the Context Epoch's baseline only, tools are sent in the registry's deterministic order with object keys in a fixed order, and history replays byte-identically (D3 `always`). Explicit `cachedContents` are out of scope; Gemini 4 does not offer them.

### D7. Tool declarations
- Each tool is a `functionDeclaration` with `parametersJsonSchema`, which takes JSON Schema as written. The OpenAPI-subset `parameters` field is not used.
- Before sending, keys Gemini rejects or ignores (`$schema`, `$id`, `examples`, `default` on objects) are removed; `$defs`/`$ref` are kept.
- Schema size and nesting limits are not published as numbers; an oversized declaration answers 400 `INVALID_ARGUMENT`. The error is `InvalidRequest` naming the tool when known. Mitigation, documented in the guide: turn on MCP tool search (deferred tool loading) or disable large MCP servers for Gemini agents. Tool schemas and descriptions are never shrunk or rewritten automatically; MCP tool search is the supported mitigation. Automatic shrinking is a possible follow-up only.
- Tool names already satisfy Gemini's `^[a-zA-Z_][a-zA-Z0-9_.:-]{0,63}$`-style limit through the registry's name rules; a name that does not is an `InvalidRequest` before sending.

### D8. Finish reasons and safety
| `finishReason` / signal | Result |
|---|---|
| `STOP` | `Finish Stop`, or `ToolCalls` when the step has calls |
| `MAX_TOKENS` | `Finish Length` |
| `SAFETY`, `PROHIBITED_CONTENT`, `BLOCKLIST`, `SPII`, `IMAGE_SAFETY`, `RECITATION` | with output already streamed: `Finish ContentFilter`, the text kept (`Refusals and finish reasons`); with none: `ContentPolicy` error. Never retried, never a fallback |
| `promptFeedback.blockReason` (no candidates) | `ContentPolicy` error naming the reason. Never retried |
| `MALFORMED_FUNCTION_CALL` | `ProviderInternal` "Gemini produced a malformed function call": retried under Retry policy, since a resample usually succeeds |
| `UNEXPECTED_TOOL_CALL` | `InvalidRequest` "Gemini called a tool that was not offered" |
| `TOO_MANY_TOOL_CALLS` | `Finish Other("too_many_tool_calls")` with the calls received so far dispatched |
| `MISSING_THOUGHT_SIGNATURE` | `InvalidRequest`, as the 400 in D5 |
| anything else (`OTHER`, `LANGUAGE`, unknown) | `Finish Other(<lowercase reason>)` |

### D9. Stream resilience
- **Idle window.** `Endpoint.idle_timeout` defaults to 600 s for the `gemini` protocol (300 s elsewhere); `providers.<p>.timeout.idle_ms` still overrides it. Gemini sends no keep-alive while thinking, and high-effort answers can be silent for more than 5 minutes.
- **Stale connection.** When sending fails before any response header because a pooled connection was closed or reset (HTTP/2 `GOAWAY`, connection reset, broken pipe on write), the request is sent once more at once on a new connection. That resend is not a Retry policy attempt and publishes no `session.retry`; a second such failure is an ordinary `Transport` failure. Waiting for headers is covered by the existing request timeout and is retried as `Transport`.

### D10. Default model
Default model resolution falls back to "the newest available tool-capable model of a provider with a credential" (`catalog/roles.rs` → `default_model`). For `google` that could select a lightweight or special-purpose model, so the `google` provider contributes only its Pro tier to that step: API IDs containing `-pro`, plus a built-in tier rule list for models whose names carry no tier. Gemini 4 names have no tier (`gemini-4-carbon`, `-argon`, `-hydrogen`); AI Studio describes Carbon as the strongest reasoning model, so the list starts with `gemini-4-carbon`. Within that set the choice is unchanged: newest `released_at`, ties by ID. With no available Pro-tier model, the provider's other tool-capable models take part. Configured, recent and explicit choices are unaffected.

### D11. Vertex AI (later phase, note only)
Vertex serves the same `generateContent` body at `https://{location}-aiplatform.googleapis.com/v1/projects/{p}/locations/{l}/publishers/google/models/{m}:streamGenerateContent?alt=sse`, authorized by an OAuth access token from Application Default Credentials (`provider-credentials` → cloud IAM chain). The adapter keeps request building, decoding and error parsing independent of the URL and auth header, so a `google-vertex` protocol can reuse them with a different `Endpoint` and token source. Not in this change.

### D12. Interactions API (future option)
The Interactions API keeps state server side (`store: true`) or returns `thought` steps to resend. It would cut request size on long Sessions, but it duplicates our event-sourced history, complicates fork, rewind and compaction, and is not needed for correctness. Revisit if request size or latency on long Sessions becomes a problem.

## Provenance

Clean-room. Behaviour comes from Google's public documentation and the Antigravity CLI changelog. Gemini CLI's quota error classifier (Apache-2.0) was read only for the idea of separating terminal from retryable quota errors; no code, identifier lists or message text is copied. Fixtures are recorded from our own live calls or written by hand from the documented shapes.

## Risks / Trade-offs

- **Undocumented quota ids.** The per-day/per-minute split relies on quota id names (`…PerDay…`, `…PerMinute…`). If Google renames them, a daily quota looks like a rate limit and is retried up to the 4-minute cap before failing; it still fails, just later. The live checklist records the ids we see.
- **Text lowering of foreign calls** loses some tool-call structure in the current turn after a model switch. It happens only after a switch in the middle of a tool loop.
- **Bigger requests with `always` replay.** Signatures are a few hundred bytes to a few KB each; the context meter counts them (3 tokens per 4 bytes).
- **Model generation from the API ID** can misjudge a new name; the `thinking_param` profile field is the override.

## Migration

No data migration. Sessions that ran Gemini through an OpenAI-compatible provider keep signatures stamped with that provider's id. Native state goes back only to the same provider and model, so switching such a Session to `google/<model>` lowers its earlier reasoning to text, and the current turn's calls follow the cross-model rule in D3.

## Follow-ups

- Automatic shrinking of oversized tool schemas, if MCP tool search proves insufficient (D7).
- Vertex AI with Application Default Credentials (D11).
- The Interactions API (D12).
- Planner/executor routing through model roles and the fallback chain.
- Moving the `gemini` feature to `stable` once the live coding baseline is published (task 6.3).
