## 1. Protocol and catalog

- [ ] 1.1 `ApiKind::Gemini` in `crates/cyber-llm/src/adapters/mod.rs`: `parse` (`gemini`, `google`, `google-genai`), `from_npm("@ai-sdk/google")`, `default_url`, `as_str`, and the `adapter()` arm.
  - Unit tests: parse aliases, `@ai-sdk/google-vertex` stays unmapped, `as_str` round-trip.
- [ ] 1.2 Base URL normalization (trailing `/`, missing `/v1beta`) and the `https`-or-loopback check.
  - Unit tests: "Base URL with a version suffix", "Plain HTTP to a remote host", a proxy prefix kept as written.
- [ ] 1.3 Add `@ai-sdk/google` to `SUPPORTED_NPM` in `scripts/refresh_models_snapshot.py`; refresh `crates/cyber-llm/snapshot/models.json.gz`.
  - Test in `tests/catalog.rs`: the snapshot lists `google` models and they resolve to `ApiKind::Gemini`.
- [ ] 1.4 Feature registry entry `gemini` (maturity `experimental`, off by default); while off, `gemini`-protocol models are unavailable with reason `experimental` and the hint `enable with features.gemini`.
  - Tests: "Experimental feature off", "Key enables Gemini once the feature is on", `cyber features list` shows `gemini experimental off (default)`.
- [ ] 1.5 Default model choice in `catalog/roles.rs`: the `google` provider contributes only Pro-tier models (`-pro` IDs plus the built-in tier list, starting with `gemini-4-carbon`) to the newest-model step, falling back to its other tool-capable models when none is available.
  - Tests: "Only a Google key", "Configured model wins", and an unchanged choice when OpenAI and Anthropic keys are present.
- [ ] 1.6 Profile defaults in `catalog/profile.rs`: the `gemini` protocol gets `reasoning_carrier: gemini_signature`, `reasoning_replay: always`; add `thinking_param` and `signature_bypass` (default `false`) profile fields with config overrides. Keep the `gemini-3` (and, once PR #144 lands, `gemini-4`) compatible-path rules unchanged.
  - Tests: the native and compatible profiles for the same model ID differ only where intended.

## 2. Adapter: requests

- [ ] 2.1 `adapters/gemini.rs`: request builder (`systemInstruction`, `contents`, `inlineData`, `functionCall`/`functionResponse` with ids, `toolConfig`, `generationConfig`), `x-goog-api-key` header, `stream`, `with_http_client`; `prepare_reserved` keeps the default refusal.
  - Golden request fixtures in `tests/adapters.rs` pinning the exact JSON for: a text Turn, a tool loop, images and a PDF.
- [ ] 2.2 Thought signature replay: same-part replay, the `FC1(+sig), FC2, FR1, FR2` order, no merge of signed parts, `always` replay, foreign current-turn calls lowered to text, no placeholder unless `signature_bypass`; with it, `skip_thought_signature_validator` on foreign current-turn calls and one `warn` line per Session.
  - Golden fixtures: "Parallel calls keep one signature", "Signature in an empty final part" (replay side), "Model switch inside a tool loop", a sequential two-step loop where both signatures are returned, "Signature bypass enabled".
- [ ] 2.3 Thinking: `thinkingLevel` vs `thinkingBudget` by generation and `thinking_param`, budget→level conversion with one `info` line, reasoning off per model, `includeThoughts`, `maxOutputTokens` default and 65,535 ceiling, temperature only when set.
  - Golden fixtures: "Level for Gemini 4", "Budget for Gemini 2.5", "Default output limit"; a test that no request ever holds both fields.
- [ ] 2.4 Tool declarations with `parametersJsonSchema`, key stripping, deterministic order.
  - Test "Stable prefix": two consecutive requests share identical `systemInstruction`, `tools` and earlier `contents` bytes.

## 3. Adapter: decoding

- [ ] 3.1 A `Decoder` impl driven by `decode_sse`: text parts → `text.delta`, `thought: true` parts → `reasoning.delta`, `functionCall` parts → `ToolCalls::start`/`set_opaque`/`finish` (whole-call arguments), signatures on non-call parts → `ReasoningItem`, finish only after `finishReason` and end of stream.
- [ ] 3.2 Usage mapping from the last `usageMetadata`.
  - Test "Cached tokens reported", plus cost through `catalog/cost.rs`.
- [ ] 3.3 Finish reason and safety mapping (design.md D8).
- [ ] 3.4 Recorded SSE fixtures under `crates/cyber-llm/tests/fixtures/gemini/` (recorded from our own calls or hand-written from the documented shapes; keys and signatures replaced), each with a decoder test:
  - parallel function calls with one signature;
  - a text answer whose signature arrives in an empty-text final part;
  - thought summaries followed by a call;
  - a safety block mid-answer (`SAFETY` after text) and a prompt block (`promptFeedback.blockReason`, no candidates);
  - `MALFORMED_FUNCTION_CALL`, `UNEXPECTED_TOOL_CALL`, `TOO_MANY_TOOL_CALLS`, `MISSING_THOUGHT_SIGNATURE`, `MAX_TOKENS`;
  - usage with cached and thought tokens.

## 4. Errors, retry and transport

- [ ] 4.1 Gemini error body parser (`ErrorInfo`, `QuotaFailure`, `RetryInfo`) and the classification table in design.md D5, in `error.rs` or the adapter, without changing `classify` for other protocols.
  - Fixture tests for each 429 variant (per-minute with `RetryInfo`, per-minute without details, per-day, spend cap), 402 prepaid balance, 400 missing signature, 400 token overflow, 400 schema, 400 `FAILED_PRECONDITION`, 403, 503.
  - Scenario tests: "Per-minute limit waits and recovers", "Daily quota stops at once", "Prepaid balance exhausted".
- [ ] 4.2 Per-provider `retry_after_cap` (`providers.<id>.retry.max_wait_ms`, 60 s default, 240 s for `gemini`) in `retry.rs`. Over the cap: fail at once in interactive Sessions; in unattended runs wait the full delay as one announced attempt, bounded by `usage_limit.max_wait` and wall deadlines. The runtime passes an `unattended` flag (the `session-runtime` → Usage limit wait definition plus background `subagent`/`workflow` Jobs, honouring `usage_limit.mode`) and the nearest deadline. Regression test for the old behaviour (shortened wait) on main first, then the fix.
  - Tests: "Advised delay beyond the cap in an interactive Session", "… in exec", "Background subagent Job waits", "Unattended wait past a deadline", "Waiting turned off"; a Workflow agent and a loop iteration wait as in exec; existing retry tests unchanged.
- [ ] 4.3 600 s idle default for `gemini`; `timeout.idle_ms` still overrides.
  - Test "Long silent thinking" with a paused-clock mock server.
- [ ] 4.4 One immediate resend on a stale pooled connection, not counted as a retry.
  - Test "Stale HTTP/2 connection" in `tests/transport_faults.rs` (connection reset before headers, then success); a second reset is an ordinary `Transport` failure.

## 5. Credentials and login

- [ ] 5.1 Pin the `google` endpoint and key variables in the catalog source integrity check; `GEMINI_API_KEY`, then `GOOGLE_GENERATIVE_AI_API_KEY`, then `GOOGLE_API_KEY`.
  - Tests "Environment key", "Variable order", "Generic Google key alone", "Tampered catalog endpoint", and another catalog provider declaring any of the three variables gets no credential.
- [ ] 5.2 `cyber providers login google`: API key method only, validated with `GET <base>/models`.
  - Tests "API key login accepted", "Invalid Gemini key".
- [ ] 5.3 Update the `/login google` refusal text in `crates/cyber-tui/src/login.rs` (no longer "await a native Gemini adapter"; point to the AI Studio key) and its test in `crates/cyber-tui/src/tests.rs`.

## 6. Live checks (need `GEMINI_API_KEY`)

- [ ] 6.1 Add ignored live tests in `crates/cyber-llm/tests/live.rs`, skipped without a key: a native Gemini 4 tool loop with parallel calls, a Gemini 2.5 loop with a budget, and a model switch from another provider mid-loop.
- [ ] 6.2 Live checklist, run by the owner with a key, results recorded in `docs/measurements/`:
  - a 10-step coding tool loop on `google/gemini-4-*` finishes with no 400;
  - parallel calls: only the first carries a signature, and the replay is accepted;
  - a high-effort answer silent for more than 5 minutes completes;
  - a forced per-minute 429 (burst requests on a free-tier key) waits the advised delay and recovers; record the quota ids and `RetryInfo` seen;
  - `cachedContentTokenCount` is non-zero by the third request of a Session;
  - the stale-connection resend is exercised by restarting a local proxy between requests.
- [ ] 6.3 Publish the live coding baseline required by "Additional native provider adapters" (`harness-evaluation`), then move the `gemini` feature to `stable` in the same change.

## 7. Follow-ups (not in this change)

- Automatic tool-schema shrinking, only if MCP tool search proves insufficient.
- Vertex AI with Application Default Credentials; the Interactions API; planner/executor routing.

## 8. Docs and checks

- [ ] 8.1 `docs/local-core-guide.md`: a "Gemini API" section beside the subscription sections (key setup, base URL, thinking levels and budgets, quota errors and what each message means, the 600 s idle default, the schema-size mitigation through MCP tool search, the experimental `features.gemini` switch, the key variable order, the unattended full wait, `signature_bypass` as a discouraged option, why subscription login is refused).
- [ ] 8.2 `docs/features.md`: a Models row update for the native Gemini adapter; README provider table, the "no native Gemini adapter yet" sentence and Known gaps; `ROADMAP.md` status.
- [ ] 8.3 Run the gate: `cargo fmt`, workspace clippy with `-D warnings`, `cargo nextest run --workspace --profile ci`, OpenAPI/SDK/CLI doc checks, `openspec validate --all --strict`, `python3 scripts/spec_lint.py` (0 errors), `python3 scripts/spec_inventory.py` (no diff). Keep cognitive complexity ≤ 15 per function; the decoder's part dispatch is the likely hotspot.
