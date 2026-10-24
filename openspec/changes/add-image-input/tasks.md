## 1. Parts, storage and the prompt API

- [x] 1.1 `cyber_llm::Content::Document` and `Content::Attachment(AttachmentRef)`. Every adapter matches them, and an unresolved `Attachment` fails the request with an invalid-request error. Test: unit tests in each adapter module.
- [x] 1.2 `runtime/attachments.rs`, storage half:
  - magic-byte sniffing (PNG, JPEG, GIF, WebP, PDF),
  - the `attachments.max_bytes` check (`AttachmentTooLargeError` with `size` and `limit`),
  - the durable blob writer at `<data>/attachments/<session_id>/<sha256>` (temporary file, sync, rename, directory sync), with reuse and removal on refusal,
  - an ephemeral directory for in-memory stores,
  - the `attachments` config schema in `cyber-core`.

  Test: `cyber-server` unit tests for sniffing and the writer.
- [x] 1.3 Normalization: `image` (PNG, JPEG, GIF and WebP features), decode limits, downscaling to `image.max_dimension`, JPEG quality 85 above `image.max_bytes`, `keep_original`; the `lopdf` page count and the page note. Test: unit tests for the 4000×3000 PNG, the decompression-bomb header and the 240-page PDF.
- [x] 1.4 `http::sessions::PromptPart` (`text`, `image`, `document`, `file`, `attachment`). Other types are refused. The prompt route gets a 64 MiB body limit, prompts are capped at 20 media parts, inline parts are stored as references before `admit_user`, and the digest hashes inline data. The three new tagged errors. Test: `crates/cyber-app/tests/attachments.rs::inline_screenshot_is_stored_once_and_sent_to_the_model`, through the AppHost HTTP server with a scripted provider that records requests. It checks the event row holds no base64, the provider got the image, a retry replays the receipt, and 25 MiB gives 413 and a `tool_call` part gives 400.
- [x] 1.5 Regenerate OpenAPI (`UPDATE_OPENAPI=1 … openapi_document_is_current`) and the TS SDK (`scripts/generate_sdk.py`). Test: `openapi_document_is_current` and the SDK type check (`npm test` in `sdk/typescript`).

## 2. File parts, model input and providers

- [x] 2.1 `ToolHost::read_attachment` (`cyber-tools/src/host/attachments.rs`), which runs the `read` tool's checks (`external_directory`, the rule, hooks, hidden paths, checkout trust) and is called from `Runtime::prompt_resources`. A denial refuses the prompt. Peer-attach prompts refuse `file` parts and accept `document` and `attachment`. Tests: `attachments.rs::file_part_asks_and_denies_like_read` (an ask answered through the real permission route; `dont-ask` denial of `/etc/…`; a `.env` image path denied by its rule) and `crates/cyber-app/tests/peers.rs::peer_prompt_refuses_file_parts`.
- [x] 2.2 The modality check at admission (`UnsupportedModalityError` with model and modality) against the catalog's `vision` and `pdf`. Test: `attachments.rs::text_only_model_refuses_images_and_pdfs`, using scripted catalog models with and without each capability.
- [x] 2.3 `attachments::project` at Turn assembly: references are resolved from the store and cached for the Drain, and the `tool-registry` placeholder is used after a model switch, plus a missing-blob placeholder. `view::strip` and `compaction::render_parts` give `[Attached <mime>: <name>]`. Tests: `attachments.rs::model_switch_and_restart_keep_the_placeholder_and_the_image` (restart the AppHost, replay, then switch to a text-only model) and `attachments.rs::overflow_compaction_strips_attachments`.
- [x] 2.4 Adapter PDF mapping: Anthropic `document`, Responses and ChatGPT `input_file`, Chat `file`; the Copilot vision header covers images only. Tests: request-body unit tests per adapter, and `attachments.rs::pdf_reaches_each_protocol` against local stub servers for `anthropic`, `openai_responses` and `openai_chat`.

## 3. Forks, children, deletion and export

- [x] 3.1 Blob linking (hard link, falling back to a copy) for forks, child tasks that carry references, and imports, before the target commits. `owned_artifacts` removes the attachment directory. Test: `attachments.rs::forks_keep_their_blobs_after_the_source_is_deleted`.
- [x] 3.2 The export document's `attachments` array. `--sanitize` omits it and redacts names. Import verifies the SHA-256 and the size before inserting. Markdown and HTML show placeholders. Tests: `crates/cyber-cli/tests/export_cli.rs::attachments_round_trip_and_sanitize` (the real `cyber` binary: export, import, a sanitized document with no `data` and no name, an HTML placeholder) and `export_cli::tampered_attachment_import_fails`.
- [x] 3.3 The subtask, delegation and inbox edit routes take `PromptPart`s through `prompt_parts::stored` (64 MiB bodies, `file` refused, blobs of a refused request removed); child admissions and edits run the modality check; `attachments::project` takes the media type from the blob's bytes. Tests: `attachments.rs::subtask_media_is_stored_by_reference_and_checked_against_the_child_model`, `attachments.rs::inbox_edits_store_media_by_reference_with_sniffed_types` and `exec_attachments.rs::a_delegated_pdf_is_stored_by_reference_and_needs_pdf_input` (the real binary with `@explore` and `--file spec.pdf`).
- [x] 3.4 The PDF page count loads only the page tree, with a 4 MiB per-stream inflate bound. Test: `normalize::tests::a_pdf_stream_inflating_past_the_bound_is_not_decoded`.

## 4. TUI

- [x] 4.1 `cyber-tui/src/attachments.rs`:
  - the chip table and labels; deleting a chip as one unit,
  - expanding chips into parts on submit (`file`, inline, `attachment`),
  - draft persistence (clipboard images as `0600` files in the TUI state directory),
  - `paste_summary` in `prefs.rs`, with `paste_summary` added to the `TUI_KEYS` of `config/validate.rs`.

  Test: `screens_attachments::large_paste_becomes_a_chip_and_submits_in_full` (a production `App`: 300 lines, `paste_summary: false`, Backspace on a chip, the restored draft).
- [x] 4.2 Drag-and-drop path detection (quoted, backslash-escaped and `file://` paths; existing image or PDF only), the client size toast, and `@` completion inserting a media chip. Test: `screens_attachments::dropped_paths_and_mentions_become_chips` (with `Screen Shot.png`, a 25 MiB file giving the toast, and `@docs/spec.pdf` giving `[PDF #1]`).
- [x] 4.3 The `Clipboard` trait with a fake, the platform readers (macOS `osascript` PNGf hex, Linux `wl-paste`/`xclip`, Windows PowerShell `Clipboard.GetImage()` as base64) using fixed argument vectors, a 5 s timeout and a byte cap; `composer.paste_image` (Ctrl+V, Alt+V on Windows) in `keys.rs`. Tests: `screens_attachments::ctrl_v_attaches_the_clipboard_image` (the fake: an image, no image, the missing-tool toast, a rebound key) and unit tests for the PNGf hex decoder and the argument vectors. The reader is picked at run time with `cfg!`, so Windows clippy compiles every reader; the one `cfg(unix)` gate (the `0600` mode) was checked by hand.
- [x] 4.4 The transcript renders `[Image: <name>]` and `[PDF: <name>]`. Server refusals (413, 415, 422, denial) keep the draft and toast the message. Test: `screens_attachments::refused_prompt_keeps_chips`, against an AppHost server with a text-only scripted model.
- [x] 4.5 Taking back a queued input (Alt+Up) replaces the draft's chips, and `restore` never keeps two chips with one label. Tests: `screens_attachments::taking_back_a_queued_image_replaces_the_draft_chips` and `attachments::tests::restore_never_holds_two_chips_with_one_label`.

## 5. Exec, docs and verification

- [x] 5.1 `exec/setup.rs::file_part`: images and PDFs are sniffed from their bytes; a local server gets a `file` part and `--attach` gets inline bytes; refusals exit with code 2 and the server's message. Test: `crates/cyber-cli/tests/exec_attachments.rs` (the real binary against a local stub provider: a screenshot on a vision model exits 0 and reaches the stub; a text-only model exits 2 with `has no image input`; a path outside the Location in `dont-ask` exits 2 with the `--add-dir` hint).
- [x] 5.2 Docs: the README feature table, `docs/features.md`, `docs/local-core-guide.md` (paste, chips, clipboard tools per OS, limits, export behavior), `docs/cli.md` regenerated, and the ROADMAP P0 row with the inventory (`scripts/spec_inventory.py`).
- [x] 5.3 `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace --profile ci`, `openspec validate add-image-input --strict`, `scripts/spec_lint.py`. Result on macOS: 4265 tests passed (11 skipped), clippy and fmt clean, `openspec validate --all --strict` 86/86, `spec_lint` 0 errors and 23 warnings (as on main), the TS SDK and plugin kit type checks and tests pass, and docs/cli.md, OpenAPI, the SDK and the ROADMAP inventory are current. Windows clippy cannot run here; every new `cfg(unix)` gate was reviewed by hand.
- [x] 5.4 The `Idempotency-Key` middleware capped every keyed body at 32 MiB, ahead of the prompt route's 64 MiB and the import route's 256 MiB limits, so a 25 MiB inline image got 400 `request body too large` instead of 413 and `cyber sessions import` failed for exports over 32 MiB. Its cap is now the import limit, so each route's own limit and error apply. Test: `attachments.rs::inline_screenshot_is_stored_once_and_sent_to_the_model` sends the 25 MiB part with an `Idempotency-Key` and expects 413 `AttachmentTooLargeError`.

## 6. Deferred

- The `read` tool still refuses images and PDFs; images are not drawn in the terminal.
- The text-only refusal message is cut off in the TUI status line when the terminal is narrow.
- A `/fs/find` reply that arrives after the last keystroke does not reopen the `@` menu until the next key (an issue on main as well, in `runner.rs`).
- Attachment blobs take the data directory's default file mode (`0644`), like the database.
