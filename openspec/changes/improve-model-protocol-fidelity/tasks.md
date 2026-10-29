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
- [x] 2.2 Anthropic tool-loop guard: thinking off when the latest tool-calling assistant message has no native item, which keeps it off for the rest of that tool loop, with one `info` log per loop.
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
- [x] 5.3 Idle timeout in the stream decoder (`adapters/mod.rs`, `DecodeState::next_chunk`; `provider.timeout.idle_ms`).
  - Test in `tests/transport_faults.rs` using a paused clock.
- [x] 5.4 Error code table, server-advised delay parsing (capped at 60 s), `usage_limit_reached` with `resets_at`, and the `response.incomplete` mapping.
  - Unit tests for each code.
  - Show `resets_at` in the TUI status and in `exec` errors, with a screen test.
  - The reset time is formatted into the error message where the adapter runs (the local server), so every client shows the same text.

## 6. Tools

- [x] 6.1 Our own Lark grammar for the patch envelope.
  - Freeform `apply_patch` on Responses when `edit_tool = freeform_patch`, plus decoding and replay of `custom_tool_call` / `custom_tool_call_output`.
  - The other adapters keep the JSON tool.
  - Done: `adapters/openai_responses/apply_patch.lark` (in the provenance scan), `Shaping.freeform_patch`; only OpenAI's own Responses backends get the grammar tool (`Backend::allows_freeform_patch`), so Copilot and any compatible gateway keep the JSON tool, and reserved requests pin it; `ResolvedModel::prefers_apply_patch()` reads the profile's `edit_tool`. Tests `freeform_patch_offers_a_grammar_tool_and_replays_custom_items`, `custom_tool_calls_decode_to_the_patch_input`, `a_freeform_patch_profile_reserves_with_the_json_tool`, `a_responses_gateway_gets_the_json_patch_tool`.
- [x] 6.2 Tolerant matching levels and heredoc-wrapped envelopes in `tools/patch.rs`.
  - Tests: "Smart quotes in context", "Ambiguous loose match", trailing-whitespace, heredoc.
  - Done: an exact match keeps taking the first place after the cursor; the looser levels must be unique (spec: "Exact hunk matches keep their order"). Tests `smart_quotes_in_context_match_ascii_quotes`, `ambiguous_loose_match_fails_naming_the_hunk`, `trailing_whitespace_and_exact_matches`, `heredoc_wrapped_envelopes_parse`.
- [x] 6.3 Shell interception of `apply_patch` heredocs (bash, powershell, `cd <dir> &&`), only when no `apply_patch` binary is on `PATH`.
  - The `edit` permission is checked, with `metadata.intercepted`.
  - Tests, including the binary-present case.
  - Done: `tools/patch_shell.rs`; the edit is authorized as `apply_patch` (tool-scoped rules apply, a shell hook's Allow does not approve it); PATH is the shell's (`sandbox.env.set.PATH` under an enforcing policy) and a login shell runs the script as written; `<<-` heredocs run as written. `metadata` on `session.tool.settled.1` and `CallState` through `ToolHost::take_call_metadata`. Tests in `cyber-tools/tests/shell_patch.rs` (bash, `cd`, edit denied, tool-scoped deny, PowerShell here-string on Windows, installed program, sandbox PATH, login shell) and `patch_shell` unit tests.
- [x] 6.4 Profile output budgets and `middle` truncation in the tool output budget.
  - Test: "Long build log for an OpenAI model". Existing truncation tests must still pass.
  - Done: `Budget::apply_for` and `Budget::apply_registered` (MCP and registered tools, with a server's `output_token_limit` as a further cap), one truncation path; the Turn model's budget rides on `Invocation.tool_output`. `truncation: default` keeps each tool's own end, `head`/`tail` force one. Tests `long_build_log_keeps_head_and_tail_for_a_middle_profile`, `default_head_and_tail_profiles_pick_the_kept_end`, `registered_results_follow_the_profile_budget`, `a_middle_profile_keeps_both_ends_of_a_long_log`.

## 7. Base prompts

- [x] 7.1 Write new base prompts (core plus the OpenAI, Anthropic, Gemini and local addenda) that cover the topics in "Base prompt coverage". Add `prompts/base/VERSION`, and start a new Epoch when the version changes.
  - Test: "Prompt version change".
  - Done: `prompts/base/*.md`, `VERSION` = 2, `Epoch::serves`; tests `prompt_version_change_starts_a_new_epoch`, `core_text_covers_every_required_topic`, `addenda_follow_each_family_edit_tool`.
- [x] 7.2 Provenance check: a script test that fails if any sentence of 12 words or more in `prompts/base/*.md` appears in the Codex prompt files. It runs against a fixture list of Codex sentences. The Codex source is not vendored; the fixture holds only short hashes.
  - Done: `scripts/prompt_shingles.py` hashes 12-word shingles of every `.md`, `.lark` and `.txt` under Codex's `codex-rs`, the text files of any `assets`/`templates`/`prompts` folder, and each string literal of its non-test Rust sources (116 061 hashes in `tests/fixtures/reference_prompt_shingles.txt`), plus 6-word shingles of its grammars (`reference_grammar_shingles.txt`); `tests/prompt_provenance.rs` and `scripts/tests/test_prompt_shingles.py` check every base prompt, any `.lark` grammar under `crates/` (also at 6 words), and the string literals of `cyber-tools/src/tools/*.rs` (tool descriptions and the messages sent back to the model).

## 8. Evidence

- [x] 8.1 Golden request fixtures in `tests/adapters.rs`, one per carrier and per Responses option set, pinning the exact JSON.
  - Done for every carrier (`golden_*` in `tests/adapters.rs`, Copilot in `tests/copilot.rs`) and for the Responses option sets (supporting model, unsupporting model, reasoning off and tools disabled).
- [x] 8.2 Live tests written (`tests/live.rs`, ignored by default, run with keys):
  - an OpenAI tool loop with encrypted reasoning replayed;
  - an Anthropic interleaved-thinking loop;
  - a DeepSeek thinking tool loop;
  - a Gemini 3 tool loop through the compatible endpoint.
  
  - Done: four `#[ignore]` tests in `tests/live.rs`, each skipped without its key. They compile.
- [ ] 8.2a Run the four live tests with provider keys; each must finish with no 400 errors.
- [ ] 8.3 Eval comparison: run the pinned-repository eval (`harness-evaluation`) on `openai/gpt-6-sol`, before and after, and record the pass rate and token use in `docs/`.

## 9. Docs and checks

- [x] 9.1 Docs: model profiles, reasoning replay, idle timeout and the new error messages in `docs/local-core-guide.md`; a row in `docs/features.md`; the ROADMAP entry.
- [ ] 9.2 Run the gate:
  - `cargo fmt`;
  - workspace clippy with `-D warnings`;
  - `cargo nextest run --workspace --profile ci`;
  - OpenAPI/SDK/CLI doc checks;
  - `openspec validate --all --strict`;
  - `scripts/spec_lint.py`, with no new warnings.
  
  Keep cognitive complexity ≤ 15 per function.
