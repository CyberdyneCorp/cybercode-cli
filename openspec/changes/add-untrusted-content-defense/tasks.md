## 1. Frame and provenance

- [x] 1.1 Add `Provenance { source, origin }` to `ToolOutcome` (`cyber-server/src/runtime/host.rs`), and the frame builder `untrusted/frame.rs` in `cyber-tools`:
  - neutralize `untrusted_content` tags in the body, case-insensitively and with whitespace;
  - escape attributes and cut them to 512 characters;
  - drop userinfo from URLs.
  
  Unit tests: forged closing tag, mixed-case and spaced variants, attribute injection with `"`, userinfo removal, `Vec<T>` kept byte for byte.
- [x] 1.2 Apply the frame in the projection step after budget truncation (`cyber-tools/src/budget.rs`), with the truncation notice after the closing tag.
  - Tests: scenario "Truncated result keeps its closing tag" for head truncation, and the same for tail truncation.
- [x] 1.3 Hidden character markers (`untrusted/frame.rs`):
  - zero-width, bidi, tag and C0 characters plus DEL, with BOM at offset 0 kept;
  - ZWJ kept between emoji;
  - `[ESC]` for ESC.
  
  Tests: scenarios "Smuggled tag characters" and "Emoji sequence kept", plus a property test that the framed body never loses a character (count markers and kept characters against the input).
- [x] 1.4 Declare provenance in `tools/web.rs`, `tools/websearch.rs` (one frame, `search:<backend>` or `search:native`), `mcp/pool.rs::output` (`mcp`, or `browser` when the server is `browser.server`, `isError` included, `structuredContent` kept), `tools/mcp.rs` and `mcp/mention.rs` (`mcp_resource`; remove `<mcp_resource>` formatting).
  - Golden output tests per source. Rewrite the existing `mcp.rs`/`mention.rs` frame tests against the new frame.
- [x] 1.5 Summarizer: pass the page in the frame instead of `<content>`, and return the summary framed with `source="webfetch"`.
  - Test: a page with a forged closing tag reaches the small-model request with one closing tag; the summary is framed.
- [x] 1.6 Overflow provenance: write `<file>.provenance.json` beside the overflow file, and frame `read`/`grep` of that file with the original source and origin.
  - Test: scenario "Overflow re-read stays framed". Regression test: a missing or corrupt sidecar frames the output with `source="overflow"` and the file path as `origin`, never unframed.

## 2. Detector

- [ ] 2.1 `untrusted/rules.rs` (`detector_version = 1`) and `untrusted/detect.rs`:
  - normalize with NFKC, lowercase, remove hidden characters, collapse whitespace;
  - the seven kinds;
  - at most 10 hits with excerpts of up to 120 characters, passed through secret redaction;
  - `signal_count`.
  
  Table-driven tests: at least 3 positive and 3 negative cases per kind, plus scenarios "Instruction override flagged", "Obfuscated phrase still matches" and "Benign page has no signals".
- [ ] 2.2 Run the detector on framed results, set `signals` in the frame (kind order as in the spec), and scan admitted cross-session messages and Channel events.
  - Tests: frame attribute order, a message with `permission_change` gets an `untrusted` record on `session.prompt.admitted.1`.
- [ ] 2.3 `scan: "all"`: scan `read`, `grep`, `bash` and `powershell` output without rewriting it.
  - Test: scenario "Project tightens scanning". Also a `read` with a bidi control yields a `hidden_text` record while the output bytes are unchanged.
- [ ] 2.4 Benchmark: a 50 KiB mixed page under 5 ms on the CI Linux x64 runner (`cargo bench` target, checked in CI with a 2x margin alarm).

## 3. Records and notices

- [ ] 3.1 Add the optional `untrusted` field to `ToolSettled` and to admitted prompts (`runtime/events.rs`).
  - Tests: round-trip, and an old event without the field reads as no signals.
- [ ] 3.2 TUI warning line, with its consequence text per Mode and setting, and excerpts when the result is expanded.
  - Snapshot tests: scenario "TUI notice under a fetch", plus `dont-ask`, `bypass` (`allow`/`deny`) and `taint: off` variants.
- [ ] 3.3 `cyber exec`: stderr `warning:` line in text mode, and `untrusted` on stream-json `tool_result`.
  - Tests: scenario "Stream JSON carries the record"; `schema_version` stays 1.
- [ ] 3.4 Web dashboard notice on tool cards and messages, rendered with `textContent`.
  - Test in `web/test`: an excerpt containing `<img src=x>` renders as text.

## 4. Taint and approvals

- [ ] 4.1 Derive the taint state in the runtime from settled and admitted events after the last promoted `source: "user"` prompt, with spawn inheritance and propagation through child results (`runtime/drain.rs`, `subtask.rs`).
  - Tests: scenarios "Taint carries into the next Turn", "Loop prompt does not clear taint", "Taint survives restart" and "Subagent taints its parent", plus `taint: any` and `taint: off`.
- [ ] 4.2 Classify risky actions in `permissions/engine.rs`: egress tools and commands, `external_directory` mutations, auto-allowed commands and secret paths.
  - Table-driven tests for each class, including `git push` inside `a && b`, an unparseable command, and `.env.example` not being risky.
- [ ] 4.3 The taint downgrade: every `allow` source becomes `ask` for risky actions, `allow_when_tainted` is kept, denies are unchanged, and the outcome is resolved per Mode, including non-interactive Sessions and `bypass`.
  - Table-driven permission tests covering every scenario of "Taint-aware approvals"; a taint ask does not count toward the 3-block fallback.
- [ ] 4.4 `permission.asked.1` `metadata.taint`, the prompt reason text, and an "always" reply that does not exempt later tainted calls.
  - Tests: event payload, and a second tainted call after "always" still asks.
- [ ] 4.5 Auto classifier: the `taint` evidence object, the trusted system line, and `tainted` on `permission.auto_decided.1` (`runtime/auto.rs`).
  - Tests: scenarios "Classifier sees the taint" and "Classifier allow still asks for egress" with a stub evaluator.

## 5. Settings and prompt

- [ ] 5.1 Add `permissions.untrusted_content` (`taint`, `bypass`, `scan`, `allow_when_tainted`) to the config schema and loader, with the project-layer tightening rule and one warning per load.
  - Tests: scenario "Project cannot switch taint off", tightening accepted, project `allow_when_tainted` ignored, change applied at the next Safe Boundary.
- [ ] 5.2 Base prompts: replace the single tool-output sentence in `prompts/base/core.md` with the untrusted-content topic, cover it in every family addendum where tool habits differ, and raise the prompt version.
  - Tests: scenario "Prompt covers the frame" for each family, and scenario "Version bump starts a new epoch".

## 6. Evaluation

- [ ] 6.1 Add `eval/fixtures/untrusted-content/`: at least 60 labeled injection samples (at least 5 per kind, across `webfetch`, `websearch`, MCP and repository files) and at least 200 benign samples, including 20 security articles.
  - A deterministic test reports recall per kind and false-positive rates, and gates on 90% recall and a 2% false-positive rate.
  - Test: scenario "Rule change lowers recall" via a mutated rule table.
- [ ] 6.2 End-to-end scripted-model scenarios for reading secrets, widening permissions and redirecting provider traffic, in `default`, `auto` (attached and not attached) and `bypass`, linked from the recovery and trust release report.
  - Test: scenario "Scripted model follows an injected page".

## 7. Docs

- [ ] 7.1 Document the frame, signals, taint, settings and the MCP resource format change in `docs/local-core-guide.md`, `docs/features.md` and the tool schema bundle descriptions. Update `ROADMAP.md` for P1 safety.
- [ ] 7.2 Run `openspec validate add-untrusted-content-defense --strict` and `python3 scripts/spec_lint.py`.
