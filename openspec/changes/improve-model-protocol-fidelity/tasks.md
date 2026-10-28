## 1. Reasoning items (data model)

- [x] 1.1 Change `Content::Reasoning` to `{ text, opaque: Option<Opaque> }`, and add `opaque` to `Content::ToolCall` (`cyber-llm/src/types.rs`).
  - Reading the old `{text, signature}` shape must keep working.
  - Add `LlmEvent::ReasoningItem { index, text, opaque }` and `ToolCallDone.opaque`.
  - Unit tests: round-trips of the old and new shapes.
- [x] 1.2 Runtime, in `runtime/drain.rs`, `model.rs` and `events.rs`:
  - accumulate reasoning per block index;
  - write one `session.reasoning.ended.1` for each block, with `index`/`opaque`, and `opaque` on `session.tool.called.1`;
  - rebuild the ordered items in `SessionState`.
  
  Regression test: an old event log replays as one item.
- [x] 1.3 Redaction and size:
  - `[opaque reasoning]` in exports (HTML/JSON), shares, logs and OTel attributes;
  - context estimate at 3 tokens per 4 bytes;
  - drop values over 1 MiB, with one warning.
  
  Tests for each.

## 2. Replay policy

- [x] 2.1 `runtime/view.rs` applies `reasoning_replay` (`never`/`tool_loop`/`always`) and the same-model guard. A `Redacted` item without text is dropped when lowered.
  - Unit tests: each policy, a provider switch, and items before and after the latest user message.
- [x] 2.2 Anthropic tool-loop guard: thinking off for one request when the latest tool-calling assistant message has no native item, with one `info` log.
  - Scenario test: "Model switch inside a tool loop".
- [x] 2.3 Compaction: summaries carry no opaque state, and the kept tail replays.
  - Test in `compaction` tests.

## 3. Adapters

- [x] 3.1 OpenAI Responses:
  - request `include`;
  - decode `output_item.done` reasoning items (`id`, `summary`, `encrypted_content`);
  - replay them in position;
  - leave out IDs the provider did not issue.
  
  Golden request fixtures, plus a decoder test with a recorded stream.
- [x] 3.2 Anthropic:
  - one item per `thinking` block, with its own signature;
  - keep `redacted_thinking {data}`;
  - replay in block order.
  
  Regression tests for "Interleaved thinking keeps every signed block" and "Redacted thinking is preserved". Both fail on main today; confirm that before fixing.
- [x] 3.3 Chat Completions carriers:
  - `reasoning_content` (a single-space placeholder when empty), `reasoning` and `reasoning_details` (concatenated by index, replayed unchanged);
  - Gemini `extra_content.google.thought_signature` on the first parallel call.
  
  A golden fixture for each carrier.
- [x] 3.4 Copilot: `reasoning_opaque` on Chat, and Responses items on Responses models.
  - Fixtures in `tests/copilot.rs`.

## 4. Model behavior profiles

- [x] 4.1 `catalog/profile.rs`:
  - the `ModelProfile` type;
  - family defaults;
  - the built-in prefix and provider rule table (OpenAI `gpt-5*`/`gpt-6*`/`o3`/`o4`, DeepSeek, Kimi, GLM, Qwen, Gemini 3, OpenRouter, Copilot, Ollama/LM Studio);
  - field-wise layering.
  
  Unit tests: each layer wins over the one before it.
- [x] 4.2 Catalog metadata: read the ChatGPT `/models` profile fields in `chatgpt/models.rs`, and `reasoning`/`tool_call` from models.dev.
- [x] 4.3 Config: `providers.<p>.models.<m>.profile` parsed and validated in `cyber-core` config, warning on an invalid key or value.
  - Tests: "Config overrides a field", and the invalid-value warning.
- [x] 4.4 Effort clamping (`nearest_lower`) for agent, workflow tier and role efforts; explicit user variants still fail.
  - Scenario test.
- [x] 4.5 `profile` in `cyber models --json` and `GET /api/v1/models`; regenerate `sdk/openapi.json`, the TS SDK and `docs/cli.md`.

## 5. Request shaping, headers, timeout, errors

- [x] 5.1 Responses shaping:
  - `reasoning.summary` gating, and `effort: "none"` only when the model lists it;
  - `text.verbosity`, `tool_choice`, `parallel_tool_calls`;
  - `prompt_cache_key` from the root Session.
  
  Add `include` to the reserved-request allowed keys (`reservation.rs`), with a test that a reserved request still passes.
  - Only the default OpenAI API URL is the API backend; a custom Responses gateway gets no routing headers (regression test).
- [x] 5.2 ChatGPT routing headers and `x-codex-turn-state` echo, scoped to one Turn.
  - Test with a mock backend: "Turn state echoed within a Turn only".
  - The Turn state spans the requests answering one prompt (its tool loop and retries) and resets when the next prompt starts; `tests/runtime.rs` covers the reset and the subagent's Session ID in retry notices.
- [x] 5.3 Idle timeout in `sse.rs` (`provider.timeout.idle_ms`).
  - Test in `tests/transport_faults.rs` using a paused clock.
- [x] 5.4 Error code table, server-advised delay parsing (capped at 60 s), `usage_limit_reached` with `resets_at`, and the `response.incomplete` mapping.
  - Unit tests for each code.
  - Show `resets_at` in the TUI status and in `exec` errors, with a screen test.
  - The reset time is formatted into the error message where the adapter runs (the local server), so every client shows the same text.

## 6. Tools

- [ ] 6.1 Our own Lark grammar for the patch envelope.
  - Freeform `apply_patch` on Responses when `edit_tool = freeform_patch`, plus decoding and replay of `custom_tool_call` / `custom_tool_call_output`.
  - The other adapters keep the JSON tool.
- [ ] 6.2 Tolerant matching levels and heredoc-wrapped envelopes in `tools/patch.rs`.
  - Tests: "Smart quotes in context", "Ambiguous loose match", trailing-whitespace, heredoc.
- [ ] 6.3 Shell interception of `apply_patch` heredocs (bash, powershell, `cd <dir> &&`), only when no `apply_patch` binary is on `PATH`.
  - The `edit` permission is checked, with `metadata.intercepted`.
  - Tests, including the binary-present case.
- [ ] 6.4 Profile output budgets and `middle` truncation in the tool output budget.
  - Test: "Long build log for an OpenAI model". Existing truncation tests must still pass.

## 7. Base prompts

- [ ] 7.1 Write new base prompts (core plus the OpenAI, Anthropic, Gemini and local addenda) that cover the topics in "Base prompt coverage". Add `prompts/base/VERSION`, and start a new Epoch when the version changes.
  - Test: "Prompt version change".
- [ ] 7.2 Provenance check: a script test that fails if any sentence of 12 words or more in `prompts/base/*.md` appears in the Codex prompt files. It runs against a fixture list of Codex sentences. The Codex source is not vendored; the fixture holds only short hashes.

## 8. Evidence

- [x] 8.1 Golden request fixtures in `tests/adapters.rs`, one per carrier and per Responses option set, pinning the exact JSON.
  - Done for every carrier (`golden_*` in `tests/adapters.rs`, Copilot in `tests/copilot.rs`) and for the Responses option sets (supporting model, unsupporting model, reasoning off and tools disabled).
- [ ] 8.2 Live tests (`tests/live.rs`, ignored by default, run with keys):
  - an OpenAI tool loop with encrypted reasoning replayed;
  - an Anthropic interleaved-thinking loop;
  - a DeepSeek thinking tool loop;
  - a Gemini 3 tool loop through the compatible endpoint.
  
  Each must finish with no 400 errors.
- [ ] 8.3 Eval comparison: run the pinned-repository eval (`harness-evaluation`) on `openai/gpt-6-sol`, before and after, and record the pass rate and token use in `docs/`.

## 9. Docs and checks

- [ ] 9.1 Docs: model profiles, reasoning replay, idle timeout and the new error messages in `docs/local-core-guide.md`; a row in `docs/features.md`; the ROADMAP entry.
- [ ] 9.2 Run the gate:
  - `cargo fmt`;
  - workspace clippy with `-D warnings`;
  - `cargo nextest run --workspace --profile ci`;
  - OpenAPI/SDK/CLI doc checks;
  - `openspec validate --all --strict`;
  - `scripts/spec_lint.py`, with no new warnings.
  
  Keep cognitive complexity ≤ 15 per function.
