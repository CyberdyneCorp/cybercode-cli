## Why

The P0 requirement `tui` → Large paste and image input is open. Pasting a 300-line log floods the composer, and there is no way to show the model a screenshot or a PDF from the TUI. The server and `cyber exec --file` already pass base64 `image` parts through, but only partly:

- every event stores the whole base64 copy,
- PDFs are not supported,
- axum's default 2 MB JSON limit refuses real screenshots,
- the 20 MiB limit, downscaling and media sniffing from `session-runtime` → Attachment normalization are not implemented,
- a model without image input gets the image anyway.

Clients that send image bytes through the API cannot use `@path` attachments either: today the TUI reads mentioned files itself, without the `read` permission.

## What Changes

- `tui`, modified "Large paste and image input":
  - Pasted text over 1,000 characters or 25 lines becomes a `[Pasted text #N +L lines]` chip. `tui.paste_summary: false` turns this off.
  - Images become `[Image #N]` chips and PDFs `[PDF #N]` chips. They come from the clipboard (`composer.paste_image`, Ctrl+V, Alt+V on Windows), from file paths dropped on the terminal, and from `@path` mentions.
  - An attachment over the limit is refused with the toast `Attachment exceeds 20 MiB`.
  - A chip is deleted as one unit. The draft keeps its chips.
- `tui`, modified "Keybindings": adds the `composer.paste_image` action.
- `server-api`, new requirement "Prompt attachment parts" (P0): `POST .../prompt` accepts these parts:
  - `text`,
  - `image` and `document` with base64 `data`,
  - `file` with a `path` the server reads under the `read` permission,
  - `attachment` with the `sha256` of a blob the Session already stores.

  The prompt route accepts bodies up to 64 MiB. Other part types are refused. The OpenAPI document and the TS SDK carry the new union.
- `server-api`, modified "Tagged error model": adds `AttachmentTooLargeError` 413, `UnsupportedAttachmentError` 415 and `UnsupportedModalityError` 422.
- `session-runtime`, modified "Attachment normalization":
  - The media type is sniffed from the bytes.
  - Images are decoded with pixel limits, then downscaled and re-encoded.
  - The PDF page note is added.
  - A prompt whose attachment needs an input the Session's model lacks (`capabilities.vision`, `capabilities.pdf`) is refused with a clear error. Nothing is dropped silently.
  - When the model changes later, history shows the `tool-registry` placeholder in place of the media.
- `storage-events`, new requirement "Attachment storage" (P0):
  - Attachments are content-addressed blobs in `<data>/attachments/<session_id>/<sha256>`. They are written durably before the event that references them, and events hold only the reference.
  - Forks, imports and child tasks link the blobs, and deleting a Session removes them.
  - The JSON export embeds them. `--sanitize` drops their bytes and names, and HTML and Markdown show a placeholder.
- `provider-catalog`, new requirement "Image and PDF input" (P0): the wire mapping for `anthropic`, `openai_responses` (and `chatgpt`), `openai_chat` and `copilot`. A stored reference that reaches an adapter is an error.
- `exec-mode`, modified "Slash commands, files and attachments":
  - `--file` sends images and PDFs as attachments. Against a local server they go as `file` parts, so the `read` permission applies. With `--attach <url>` the CLI uploads the bytes.
  - The limit is the server's `attachments.max_bytes`. The 10 MiB data-URL rule is gone.

## Non-goals (deferred)

- **Rendering images in the terminal (sixel or kitty graphics).** Support differs across terminals, multiplexers and SSH, and the chip with name and size is enough to know what was attached. The transcript shows `[Image: <name>]`.
- **Binary attachments other than images and PDF.** The catalog has input capabilities only for images and PDFs (`vision`, `pdf`), and no supported provider takes arbitrary files as input. Other binaries are refused with `UnsupportedAttachmentError`.
- Media returned by the `read` tool. It still refuses images and PDFs, and the `builtin-tools` read requirement stays open. This change adds the storage and adapter mapping that work will reuse.
- Text `--file` and text `@path` mentions keep their current client-side read. Moving them to server `file` parts changes the text the model sees, so that is a separate change.
- `cyber db backup --include-artifacts` covering the attachment directory. The bundle command does not exist yet.

## Capabilities

### Modified Capabilities

- `tui`: Large paste and image input, Keybindings.
- `server-api`: Prompt attachment parts (new), Tagged error model.
- `session-runtime`: Attachment normalization.
- `storage-events`: Attachment storage (new).
- `provider-catalog`: Image and PDF input (new).
- `exec-mode`: Slash commands, files and attachments.

## Impact

`crates/cyber-llm` (`types::Content` gains `Document` and `Attachment`; the adapters `anthropic`, `openai_responses`, `openai_chat`, `copilot` and `chatgpt`), `crates/cyber-server` (new `runtime/attachments.rs` for the store, sniffing, normalization, projection and modality check; `http/sessions.rs` for `PromptPart` and the body limit; `runtime/mod.rs` → `prompt_resources` for `file` parts; `runtime/view.rs` and `runtime/compaction.rs`; `runtime/fork.rs` and `runtime/deletion.rs`; `http/export` and its sanitizer; `http/peer/attach.rs`; `http/openapi.rs`), `crates/cyber-tools` (new `host/attachments.rs`, which reads a `file` part through the `read` tool's checks), `crates/cyber-client` (export placeholders), `crates/cyber-tui` (new `attachments.rs` for chips, path detection and the `Clipboard` trait with its platform readers; `composer.rs`, `app.rs` → `on_paste`, `perform.rs` → `parts`, `keys.rs`, `model.rs`, `prefs.rs`), `crates/cyber-cli` (`commands/exec/setup.rs` → `file_part`), `crates/cyber-core` (`config/validate.rs` adds the `paste_summary` TUI key, plus the `attachments` schema), and the TS SDK (regenerated). New dependencies: `image` (PNG, JPEG, GIF and WebP features only) and `lopdf` (page count), both pure Rust. Docs: README feature table, `docs/features.md`, `docs/local-core-guide.md`, `docs/cli.md` (generated) and the ROADMAP P0 row.
