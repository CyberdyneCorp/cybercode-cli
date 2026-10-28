## Why

Codex CLI gets noticeably better results from OpenAI models than we do with the same models. Reading its source (Apache-2.0, `openai/codex`, October 2026) shows the gap is not one feature but how faithfully it speaks each model's protocol. Most of those lessons apply to other providers too:

- **Reasoning is thrown away between tool calls.** `session-runtime` → Reasoning lowering across models already says provider reasoning state (signatures, encrypted reasoning) is sent back natively to the same model. Only Anthropic does it, and only partly:
  - The Responses adapter never asks for `reasoning.encrypted_content` and drops reasoning on replay. With `store: false`, GPT-5/6 reasons again from scratch after every tool result.
  - The Chat Completions adapter reads `reasoning_content` but never sends it back. DeepSeek thinking mode answers 400 when it is missing from a tool loop; Kimi K2 and GLM lose their reasoning across tool calls.
  - Gemini 3 (OpenAI-compatible endpoint) attaches a thought signature to each tool call in `extra_content.google`, and rejects a later request without it. OpenRouter carries reasoning in `reasoning_details`; Copilot in `reasoning_opaque`.
  - An assistant message stores one reasoning string and one signature. Anthropic interleaved thinking produces several signed blocks per response, and `redacted_thinking` blocks are dropped. Both break the next request of a tool loop with thinking on.
- **The model gets prompts and tools it was not tuned for.** Our OpenAI base prompt is one sentence; Codex's is a full operating manual (editing discipline, autonomy, progress updates, final-answer shape, parallel reads). Codex offers `apply_patch` as a freeform tool with a grammar, accepts patches sent through the shell, and matches hunks tolerantly. Ours is a strict JSON tool.
- **Tool output is shaped differently.** Codex truncates command output in the middle, keeping the start and the end, against a token budget per model.
- **Requests leave out fields the models use.** Missing: `text.verbosity`, `parallel_tool_calls`, a session-derived `prompt_cache_key` on every request, reasoning summaries only where supported, session and turn routing headers (better cache hits on the ChatGPT backend), and a stream idle timeout.
- **Errors are classified coarsely.** OpenAI's `context_length_exceeded`, `usage_not_included`, `server_is_overloaded`/`slow_down` with "try again in Ns", ChatGPT `usage_limit_reached` with `resets_at`, and an `interrupted` incomplete response all become generic kinds.

## What Changes

- **Reasoning items** (`session-runtime`, added "Reasoning items" and "Reasoning replay policy").
  - An assistant message keeps an ordered list of reasoning items. Each has its text and the provider's opaque state: an encrypted blob, a signature, a redacted block, or a per-tool-call thought signature.
  - The items are persisted in the existing `session.reasoning.ended.1` and `session.tool.called.1` events as optional fields, so old events still read.
  - Replay follows the model's policy: `never`, `tool_loop` (assistant messages after the latest user message) or `always`.
  - Every adapter learns its provider's carrier field:
    - OpenAI Responses: reasoning items with `encrypted_content`;
    - Anthropic: `thinking` and `redacted_thinking` blocks, in order;
    - Chat Completions: `reasoning_content`, `reasoning` or `reasoning_details`;
    - Gemini through the OpenAI-compatible endpoint: `extra_content.google.thought_signature` on tool calls;
    - Copilot: `reasoning_opaque`.
- **Model behavior profiles** (`provider-catalog`, added "Model behavior profiles"). This is a typed, per-model record that replaces scattered family checks. It covers:
  - reasoning replay policy and carrier;
  - verbosity support and default;
  - reasoning summary support;
  - parallel tool calls;
  - edit tool shape (`freeform_patch`, `json_patch`, `edit`);
  - tool-output budget and truncation mode;
  - effort levels and how an unsupported effort is clamped.
  
  The record is resolved in layers: built-in defaults by family, then built-in rules by model-ID prefix, then provider catalog metadata (the ChatGPT `/models` fields), then config `providers.<p>.models.<m>.profile`. `cyber models --json` shows it.
- **Request shaping** (`provider-catalog`, added "Responses request shaping", "Session routing headers" and "Stream idle timeout").
  - Responses requests send `include: ["reasoning.encrypted_content"]`, `parallel_tool_calls`, `tool_choice: "auto"`, `text.verbosity` where supported, `reasoning.summary` only where supported, and a session-derived `prompt_cache_key`.
  - The ChatGPT backend also gets `session-id`, `thread-id`, `x-client-request-id`, and the `x-codex-turn-state` value echoed back within a Turn.
  - Every streaming adapter fails a stream that goes 300 s with no event as a retryable `Transport` error.
- **Provider error codes** (`provider-catalog`, added "Provider-specific error codes"). This is a table of known in-stream and HTTP error codes, mapped onto the existing typed reasons. It adds a parsed server retry delay, and a `resets_at` for subscription usage limits that the TUI shows.
- **Base prompt coverage** (`system-context`, added "Base prompt coverage"). We write our own base prompts. They cover the same topics Codex's do for OpenAI models, and are adapted for each family. No text is copied from Codex.
- **Freeform patch tool and tolerant patching** (`builtin-tools`, added "Freeform apply_patch", "Tolerant patch matching" and "apply_patch through the shell").
  - Models whose profile says `freeform_patch` receive `apply_patch` as a custom tool with our own grammar for the existing envelope.
  - For every model, hunk matching falls back from exact to trailing-whitespace, then surrounding-whitespace, then typographic-punctuation equivalence.
  - A `bash` command that is only an `apply_patch` heredoc is applied as a patch, with the same permissions.
- **Middle truncation for command output** (`tool-registry`, added "Profile output budgets"). A profile can set a token budget and `middle` truncation. Then command output keeps its head and tail around an omission marker, and the full text still goes to the overflow file.

## Non-goals (follow-up changes)

- **The Responses WebSocket transport.** This covers incremental input with `previous_response_id`, prewarm, and the HTTP fallback. It is OpenAI-only and needs its own transport design, so it goes in a later change, `add-openai-native-transport`.
- **zstd request bodies.** Also OpenAI-only, deferred to the same change.
- **Server-side compaction** (the `compaction_trigger` input item and the encrypted `compaction` item). It changes the `compaction` capability and the Context Epoch model, so it goes in its own change.
- **Code mode** (a JavaScript `exec` tool that nests other tools). This is a separate tool runtime.
- **Hosted provider tools** (OpenAI `web_search` and image generation). We have our own `websearch`. Hosted tools need billing and permission rules.
- **Model-upgrade nudges and migration screens.**
- **Using the base instructions the ChatGPT backend serves.** They are written for and name Codex, so we use our own prompts.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

All of the following are ADDED requirements. No existing requirement is modified, so this change cannot conflict with open changes.

- `session-runtime`: "Reasoning items" and "Reasoning replay policy". These refine Reasoning lowering across models.
- `provider-catalog`:
  - "Model behavior profiles";
  - "Responses request shaping";
  - "Session routing headers";
  - "Stream idle timeout";
  - "Provider-specific error codes".
- `system-context`: "Base prompt coverage".
- `builtin-tools`: "Freeform apply_patch", "Tolerant patch matching" and "apply_patch through the shell".
- `tool-registry`: "Profile output budgets".

## Impact

- **Code:**
  - `crates/cyber-llm`:
    - `types.rs`: reasoning items and tool-call opaque state;
    - every adapter;
    - `catalog/` (new `profile.rs`);
    - `error.rs`;
    - `sse.rs` (idle timeout);
    - `chatgpt/models.rs` (profile fields).
  - `crates/cyber-server/src/runtime`:
    - `drain.rs`, `model.rs`, `events.rs`, `view.rs`: persist and replay items under the policy.
  - `crates/cyber-tools/src/tools`:
    - `patch.rs` (grammar, tolerant matching);
    - `bash.rs` (patch interception, middle truncation).
  - `crates/cyber-core`: `prompts/base/*.md` rewritten, `prompts.rs`.
- **Data:** new optional fields on `session.reasoning.ended.1` and `session.tool.called.1`. Older events replay with one text-only item. Opaque state is stored as written and is redacted from exports and logs.
- **API, SDK and CLI:** `cyber models --json` and `GET /api/v1/models` gain a `profile` object, so the OpenAPI document and the SDK are regenerated. No other surface changes.
- **Licensing:** clean-room only. No code, prompt text or grammar is copied from Codex; see design.md → Provenance.
- **Docs:** `docs/local-core-guide.md` (model profiles, reasoning replay), `docs/features.md` and `ROADMAP.md`.
