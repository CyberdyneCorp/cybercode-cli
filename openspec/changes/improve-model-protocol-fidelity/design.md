## Context

The research behind this change is a read of `openai/codex` at `4d3ed3f` (2026-10-27), mainly `codex-rs/core/src/client.rs`, `codex-api`, `protocol/src/openai_models.rs`, `models-manager`, `apply-patch` and `utils/output-truncation`. It also draws on public provider documentation:
- [DeepSeek thinking mode](https://api-docs.deepseek.com/guides/thinking_mode/)
- [Kimi K2 thinking](https://platform.kimi.ai/docs/guide/use-kimi-k2-thinking-model)
- [Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)
- [OpenRouter reasoning tokens](https://www.openrouter.ai/docs/guides/best-practices/reasoning-tokens)

Where we stand today:
- `LlmRequest`/`Content::Reasoning { text, signature }` holds one string and one signature for each assistant message (`crates/cyber-llm/src/types.rs`). The runtime builds that from one accumulator per step (`runtime/drain.rs`: `acc.reasoning`, `acc.signature`).
- `runtime/view.rs` sends reasoning natively when the provider and model match, and otherwise lowers it to text. This is the place where a replay policy belongs.
- Adapters:
  - `openai_responses.rs` sends `store:false` but leaves out `include`, and drops `Content::Reasoning`.
  - `openai_chat.rs` decodes `reasoning_content`/`reasoning` and never sends it back.
  - `anthropic.rs` replays one signed `thinking` block and ignores `redacted_thinking`.
- Base prompts are one line per family (`crates/cyber-core/prompts/base/*.md`).

## Goals / Non-Goals

**Goals:**
- Every provider gets back exactly the reasoning state it needs, in the order it produced it.
- Model-specific knowledge lives in one typed profile, not in `if family == …` branches.
- Each model gets the tool shapes and output format it handles best.
- Error handling is detailed enough for retries and for messages the user can act on.

**Non-goals:** see proposal.md: WebSocket transport, zstd, server-side compaction, code mode, hosted tools.

## Decisions

### D1. Reasoning items and opaque state
`Content::Reasoning` becomes the item type:

```
Content::Reasoning { text: String, opaque: Option<Opaque> }
Opaque { provider: String, model: String, kind: OpaqueKind, data: String, id: Option<String> }
OpaqueKind = EncryptedReasoning | Signature | Redacted | Details
```

- A message holds as many items as the provider produced, in stream order. They can be interleaved with text and tool calls: Anthropic interleaved thinking is `thinking → tool_use → thinking → tool_use`, and order matters for its signature checks.
- `Content::ToolCall` gains `opaque: Option<Opaque>` for state that belongs to one call. This is the Gemini thought signature in `extra_content.google.thought_signature`. Only the first parallel call carries one, so we keep it on that call.
- `signature` serializes as before. Reading an old `{text, signature}` produces one item with `kind: Signature`, so stored Sessions still replay to Anthropic.
- **Events:**
  - `session.reasoning.ended.1` gains optional `items: [{text, opaque?}]` and `index`. A step with several reasoning blocks writes one event per block, each with its stream `index`.
  - `session.tool.called.1` gains optional `opaque`.
  - Readers that ignore unknown fields keep working, so the version suffix stays `.1`.
- The `provider` and `model` on `Opaque` repeat the message's own. They let a forked or imported message keep its provenance, and the guard in D3 compares them.
- **Size and privacy:**
  - Opaque data is stored as written. It is base64 or provider-signed, never decoded.
  - It is left out of exports, shares, logs and OTel spans, where `[opaque reasoning]` stands in for it. Its byte length counts towards context estimates with the rule `len * 3 / 4` tokens.
  - A blob over 1 MiB is dropped when it is received. That item is then replayed as text, and one warning is logged.

*Alternative rejected:* storing the provider's raw response items and replaying them unchanged. That couples storage to each provider's wire format and makes cross-provider lowering harder. The neutral item keeps `view.rs` provider-agnostic.

### D2. Carriers per adapter

| Adapter / provider | Read from the stream | Sent back as |
|---|---|---|
| OpenAI Responses, ChatGPT | `response.output_item.done` with `item.type == "reasoning"`: `id`, `summary[].text`, `encrypted_content`. Request `include: ["reasoning.encrypted_content"]`. | `{type:"reasoning", id, summary:[{type:"summary_text", text}], encrypted_content}`, in order before the function calls it preceded |
| Anthropic | `content_block_start` `thinking` / `redacted_thinking`, and `signature_delta`, per block index | `thinking {thinking, signature}` / `redacted_thinking {data}`, in block order |
| Chat Completions, `reasoning_content` carrier (DeepSeek, Kimi, GLM, Qwen, vLLM) | `delta.reasoning_content` | the assistant message's `reasoning_content` string; a single space when the text is empty, because some servers reject an empty string |
| Chat Completions, `reasoning` carrier (Ollama, LM Studio) | `delta.reasoning` | `reasoning` |
| Chat Completions, `reasoning_details` carrier (OpenRouter) | `delta.reasoning_details[]`, concatenated per index | `reasoning_details` array, unchanged |
| Chat Completions at Gemini | `tool_calls[].extra_content.google.thought_signature` | the same field on the same call |
| Copilot | `delta.reasoning_opaque` (Chat) or the Responses items | `reasoning_opaque` on the assistant message (Chat) or Responses items |

- The carrier is a profile field (D4). An adapter never guesses it from the base URL, except through the built-in prefix rules, which can be overridden.
- An item whose `id` has no known provider prefix is sent without an `id`. This follows the behaviour Codex relies on: the backend rejects ids it did not issue, such as ids from a fork.

### D3. Replay policy
`reasoning_replay` takes one of three values:
- `never`: lower everything to text, which is today's behaviour for other providers;
- `tool_loop`: native items for assistant messages after the latest user message; earlier reasoning is dropped, not lowered, because these providers ignore it or count it twice;
- `always`: native items for the whole history.

- The default is `always` for OpenAI Responses and Anthropic, matching how both providers document stateless replay. It is `tool_loop` for the Chat Completions carriers and Gemini, and `never` when the profile has no carrier.
- **Native only for the same model.** Items are sent natively only when `opaque.provider/model` equals the Turn's provider and model, which is today's Reasoning lowering rule. Otherwise:
  - an item with text is lowered to text;
  - a `Redacted` item, which has no text, is dropped.
- **Anthropic tool-loop guard.** When thinking is enabled and the latest assistant message (with tool calls) has no native item, an Anthropic request can't be built. This happens after a model switch in the middle of a loop. Thinking is then turned off for that single request, and one `info` log line says why. Without the guard the provider answers 400.
- **Compaction keeps no opaque state.** A compaction summary carries none. Items in the kept tail are still replayed.

### D4. Model behavior profiles
`ModelProfile` lives in `crates/cyber-llm/src/catalog/profile.rs`:

```
reasoning_replay: never|tool_loop|always       reasoning_carrier: none|responses|anthropic|reasoning_content|reasoning|reasoning_details|gemini_signature|copilot_opaque
reasoning_summary: bool                        verbosity: { supported: bool, default: low|medium|high|null }
parallel_tool_calls: bool                      edit_tool: freeform_patch|json_patch|edit
tool_output: { tokens: u32|null, truncation: head|tail|middle }
efforts: [..], effort_clamp: nearest_lower|fail
```

**Resolution layers, last one wins per field:**
1. Family defaults (`openai`, `anthropic`, `gemini`, `local`, `default`).
2. Built-in model-ID prefix rules: a short table, for example `deepseek-` → `reasoning_content`/`tool_loop`, `kimi-` → `reasoning_content`/`tool_loop`, `gemini-3` → `gemini_signature`, `gpt-5`/`gpt-6`/`o3`/`o4` → `freeform_patch` with `middle` truncation at 10 000 tokens and verbosity supported. The provider ID can scope a rule, so OpenRouter models get `reasoning_details` whatever the model ID.
3. Catalog metadata: the ChatGPT `/models` fields `support_verbosity`, `default_verbosity`, `supports_reasoning_summary_parameter`, `apply_patch_tool_type`, `truncation_policy` and `supported_reasoning_levels`, and `reasoning`/`tool_call` capabilities from models.dev.
4. Config: `providers.<p>.models.<m>.profile`. Each field can be overridden and is validated, and an unknown key warns.

**Rules:**
- **Effort clamping.** An effort the model doesn't list, coming from an agent profile or a workflow tier, is clamped to the nearest lower listed level, with one `info` log line. An explicit user variant still fails as before (`VariantUnavailableError`).
- **Context.** The profile changes nothing about the context or the system prompt prefix. So switching between two models whose profiles differ only in request shaping does not start a new Context Epoch. A different `edit_tool` changes the tool list, and tool lists already follow per-Turn materialization.

### D5. Responses request shaping
These apply to `openai_responses.rs` and therefore ChatGPT, and to Copilot's Responses models:
- `include` always contains `"reasoning.encrypted_content"` when the model has the `reasoning` capability.
- `reasoning.summary`:
  - sent as `"auto"` only when `reasoning_summary` is true, and dropped otherwise;
  - with `Reasoning::Off`, `reasoning` is `{effort:"none"}` only when the model lists `none`, and the field is left out otherwise.
- `text.verbosity`: the profile default (which config can set). Sent only when supported, and a configured default on an unsupported model warns once.
- `parallel_tool_calls: <profile>` and `tool_choice: "auto"` whenever tools are present; with tools disabled, `tool_choice` stays `"none"` as today.
- `prompt_cache_key`: sent on every request, taken from the root Session ID (`<root>` or `<root>:<agent>` for subagents). The `cache` setting is still respected: `cache: false` leaves it out.
- **Reserved requests.** The reserved-request contract (`reservation.rs`) must still accept the result. `include` and `text.verbosity` are added to its allowed keys. `parallel_tool_calls` and `tool_choice` are already allowed.

### D6. Routing headers and turn state (ChatGPT backend)
- **Request headers:**
  - `session-id` (the root Session ID);
  - `thread-id` (the Session ID);
  - `x-client-request-id` (the Session ID);
  - `x-openai-subagent`: `compact` for compaction, `title` or `recap` for those calls, and the agent name for a subagent.
- **Turn state:** the first response's `x-codex-turn-state` header is kept in the Turn's drain state. It is sent back on every retry and follow-up request in that Turn, and dropped when the Turn ends. It is never persisted.
- **Scope:** these headers go only to the ChatGPT backend. API-key OpenAI gets `session-id` alone, which the Responses API ignores, and other providers get none.

### D7. Stream idle timeout
- `sse.rs` wraps the event stream in a timer that resets on every byte. The window is `provider.timeout.idle_ms`, default 300 000; `0` turns it off.
- When the timer expires, the stream ends with `Transport("stream idle for 300s")`. The retry policy treats that as retryable when no output was received, and otherwise as a mid-stream failure, as today.

### D8. Error codes
A table in `error.rs` maps known codes onto the existing reasons. Nothing changes in the reason set itself.

| Code | Reason | Extra |
|---|---|---|
| `context_length_exceeded` | `ContextOverflow` | |
| `insufficient_quota`, `*_spend_limit_exceeded`, `credit_balance_exhausted` | `QuotaExceeded` | |
| `usage_limit_reached` (429 body `error.type`) | `QuotaExceeded` | `resets_at`, `plan_type` |
| `usage_not_included` | `Authentication` | "plan does not include this model" |
| `server_is_overloaded`, `slow_down`, `policy_capacity_unavailable` | `ProviderInternal` | server delay |
| `rate_limit_exceeded` | `RateLimit` | server delay |
| `invalid_prompt`, `*_policy` | `ContentPolicy` | |
| `previous_response_not_found` | `InvalidRequest` | reserved for the WebSocket change |

- **Server delay.** Parsed from `Retry-After`/`retry-after-ms`, then from the message pattern "try again in <n>(s|ms|seconds)". The retry policy uses it in place of its backoff and caps it at 60 s.
- **`resets_at`.** Shown by the TUI and `exec` as "usage limit reached; resets at 14:20". The fallback chain already treats `QuotaExceeded` as a reason to fall back.
- **`response.incomplete`:**
  - `interrupted` finishes as `Stop`.
  - `content_filter` keeps `ContentFilter`.
  - `max_output_tokens` keeps `Length`.
  - Any other reason fails the step as `ProviderInternal` with the reason text, instead of being treated as a silent `Stop`.

### D9. Base prompt coverage
We write our own prompt for each family. Each covers, in its own words:
- identity and scope;
- reading before editing, and the smallest sufficient change;
- the edit tool to use, and never editing through shell redirection;
- searching with `rg` and batching independent reads and searches in parallel;
- autonomy: carry the task through to verified completion, and ask only when blocked on a real decision;
- respecting a dirty worktree and never reverting changes the agent did not make;
- destructive-action caution;
- short progress updates during long work;
- the final answer: lead with the result, refer to files as `path:line`, and no filler.

OpenAI and local prompts emphasize the patch tool and parallel reads. Anthropic's emphasizes the `edit` tool and parallel tool use. Gemini's emphasizes small sequential edits and explicit verification.

The prompts are versioned (`prompts/base/VERSION`). A version change starts a new Context Epoch on the next Turn, the same way a family change does.

### D10. Freeform `apply_patch` and tolerant patching
- **Freeform tool.** A profile with `freeform_patch` offers `apply_patch` as `{type:"custom", name:"apply_patch", format:{type:"grammar", syntax:"lark", definition}}`. The adapter forwards it only on OpenAI Responses. Any other adapter falls back to the JSON tool. The grammar is written by us from our own spec of the envelope (`builtin-tools` → apply_patch tool). The decoder reads `custom_tool_call` items (`response.custom_tool_call_input.delta`/`.done`) into a `ToolCall` with `input: {"patch": <raw>}`, and replays the result as `custom_tool_call_output`.
- **Tolerant matching.** Applies to every model. Each hunk's context and removed lines are searched in this order, and the first single match wins:
  1. exact;
  2. trailing whitespace ignored;
  3. surrounding whitespace ignored;
  4. typographic punctuation folded (curly quotes, dashes, non-breaking spaces).
  
  More than one match at the first level that matches fails, as an ambiguous hunk. The envelope parser also accepts a patch wrapped in a heredoc (`<<'EOF' … EOF`) or in surrounding whitespace.
- **Through the shell.** A `bash` or `powershell` command whose whole script is `apply_patch <<'X' … X` (optionally `cd <dir> &&`) is run as `apply_patch` instead. It uses the same permission checks and the same result. The call is still recorded as `bash`, with `metadata.intercepted: "apply_patch"`.

### D11. Profile output budgets
When the profile sets `tool_output.tokens`, the model-facing budget is the smaller of the byte limit and `tokens * 4` bytes.
- With `truncation: middle`, the kept text is head and tail in a 1:1 split on line boundaries, around `…[<n> lines / <m> bytes omitted; full output at <path>]…`.
- `head` and `tail` keep today's behaviour.

The overflow file and `output_paths` work as today. The marker stays one line, so `builtin-tools` → Tool output formats are stable contracts keeps its parse.

## Provenance

- Codex is Apache-2.0, which would allow copying with notice. Even so, at the owner's request this change is clean-room.
  - We learned the behaviour (which fields, which ordering, which failure modes) from reading the source and public API docs.
  - We write all code, prompt text and the patch grammar ourselves.
  - No Codex file, prompt or grammar is vendored or pasted.
- Reviews check this rule. Task 9.2 includes a check that no sentence of 12 words or more in our prompts appears in Codex's prompt files.

## Risks / Trade-offs

- **Bigger requests.** Encrypted reasoning makes requests larger, though prompt caching offsets it. The context meter counts the blobs (D1), so auto-compaction triggers earlier than it does today, which is correct.
- **Provider drift.** The built-in prefix rules can go stale. Config overrides (D4 layer 4) are the escape hatch, and the live tests (task 8) catch drift.
- **More moving parts in replay.** Golden request fixtures for each carrier pin the exact JSON (task 8.1).
- **Shell interception surprise.** A user who really meant to run an `apply_patch` binary is affected. The interception applies only when no `apply_patch` executable is on `PATH`.

## Migration

- Old events read as single items, so no data migration is needed.
- The first Turn after the upgrade on an old Session replays its stored Anthropic signature as before. OpenAI Sessions have no stored blobs and start collecting them.
