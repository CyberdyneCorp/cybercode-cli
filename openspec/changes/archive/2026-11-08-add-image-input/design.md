## Context

What exists today:

- `cyber_llm::Content::Image { media_type, data }` carries base64 bytes. The `anthropic`, `openai_responses` (which `chatgpt` reuses through `build_body`) and `openai_chat` adapters map it. `copilot` adds `Copilot-Vision-Request: true` when a request has an image.
- `POST /sessions/{id}/prompt` takes `PromptBody.parts: Vec<Content>`. Any variant is accepted, including `tool_call`. The parts are stored as they arrive in `session.prompt.admitted` (`runtime::events::Admitted`) and in `Entry::User`, so every replay and fork copies the base64.
- axum's `Json` extractor keeps its default 2 MB limit, so a real screenshot is refused with 413 before any handler runs.
- `runtime/view.rs::strip` and `runtime/compaction.rs::render_parts` already turn images into `[Attached <mime>]` after an overflow compaction.
- `cyber exec --file` reads the file on the client. PNG, JPEG, GIF and WebP become inline `image` parts, and anything else must be UTF-8 text.
- The TUI composer is plain text. `App::on_paste` inserts the whole paste. `perform::parts` reads `@path` mentions on the client as text, and `model.rs` renders an image part as `[Image]`.
- Capabilities `vision` and `pdf` exist in the catalog (models.dev modalities, Copilot `supports.vision`, config `capabilities`), but nothing reads them.
- Admission-time reads that respect permissions already exist. `Runtime::prompt_resources` expands command templates, and `BuiltinHost::read_reference` (`cyber-tools/src/host/template_expand.rs`) runs `@` references through the real `read` tool. That covers `external_directory`, `read` rules, hooks and the hidden-path sandbox, and an `ask` reaches the attached client while the prompt request waits.
- Content-addressed artifacts are stored in `browser/evidence.rs` (`<data>/browser/artifacts/<session_id>/<sha256>.<ext>`), and `runtime/deletion.rs::owned_artifacts` removes them with their Session.

## Decisions

### Parts: what clients send and what is stored

The HTTP input becomes its own type, `http::sessions::PromptPart`. It is no longer `Content`:

| part | fields | handled |
|---|---|---|
| `text` | `text` | unchanged |
| `image` | `media_type`, `data` (base64), `name?` | decoded, checked and stored at the HTTP edge |
| `document` | `media_type` (`application/pdf`), `data`, `name?` | same |
| `file` | `path`, `name?` | resolved at admission through the `read` tool (below) |
| `attachment` | `sha256` | must name a blob in this Session's store; used to resubmit chips restored by rewind |

Any other `type` is `InvalidRequestError`. Today a client could send `tool_call` or `reasoning` parts as user input, and refusing them is intended.

`cyber_llm::Content` gains two variants:

- `Document { media_type, data, name? }`: base64 PDF bytes for the adapters, like `Image`.
- `Attachment(AttachmentRef { sha256, media_type, name, size, width?, height?, pages? })`: the durable reference. This is the only media form written to events, so `Admitted.parts` and `Entry::User.parts` stay small. Older events with inline `Image` parts still replay unchanged.

Adapters never receive an `Attachment`: the runtime resolves it first (see Projection). An adapter that does see one returns `LlmError::InvalidRequest("unresolved attachment")`. This fails closed, and the exhaustive `match` keeps every adapter honest.

The admission digest is computed over the input parts, with inline `data` replaced by its SHA-256. An idempotent retry of the same bytes therefore gets the same digest and the same receipt, and the digest never hashes 27 MB of base64.

### Limits and sniffing (server side)

- **Body size:** the `prompt`, peer-attach `prompt`, `subtask`, delegation start and inbox edit routes get `DefaultBodyLimit::max(64 MiB)`. Every other route keeps the default.
- **Other routes with media:** a subtask's or delegation's `attachments` and an inbox edit's `parts` are `PromptPart`s too, stored by `prompt_parts::stored` against the Session posted to (`file` parts refused). A child adopts its parent's blobs at admission and its model must take them; an edit runs the modality check before its hooks.
- **Per attachment:** each decoded attachment is limited to `attachments.max_bytes` (default 20 MiB), and larger ones fail with `AttachmentTooLargeError` 413 `{ size, limit }` and the message `Attachment exceeds 20 MiB`. A `file` part is checked against its metadata before it is read.
- **Per prompt:** at most 20 media parts.
- **Media type:** taken from magic bytes (PNG, JPEG, GIF, WebP, `%PDF-`), never from the client's `media_type` or the file extension. Anything else is `UnsupportedAttachmentError` 415.
- **Decompression bombs:** images are decoded with `image::Limits`: at most 100 megapixels and 512 MiB of allocation. An image above that fails with `UnsupportedAttachmentError` before any pixel buffer is allocated. A PDF's page count is read with lopdf keeping only the catalog, page tree nodes and object streams, and no object or cross-reference stream may inflate past 4 MiB; a PDF that cannot be read within that has an unknown page count and is attached as is.

### Normalization (`session-runtime` → Attachment normalization)

`runtime/attachments.rs::normalize` runs in `spawn_blocking`:

- **Images:**
  - An image larger than `image.max_dimension` (2000 px) on either side is resized to fit, keeping the aspect ratio (Lanczos3), and re-encoded as PNG.
  - An image over `image.max_bytes` (5 MiB), after resizing or not, is re-encoded as JPEG quality 85.
  - An animated GIF within both limits is kept as it is. Otherwise only its first frame is used.
  - With `attachments.keep_original`, the original blob is also stored, and the reference records `original`.
- **PDFs:** `lopdf` counts the pages.
  - Above `pdf.max_pages` (100), the model gets the PDF followed by the text `[<name>: <n> pages; only pages 1-100 are expected to be read unless the user names others]`.
  - A PDF `lopdf` cannot parse is attached as is, without the note. The page cap is advisory and the provider's own limit still applies.

The process is deterministic, so equal input always yields equal blobs.

### Storage (`storage-events` → Attachment storage)

- **Layout:** `<data>/attachments/<session_id>/<sha256>`, without an extension, because the media type is in the reference.
- **Writes:** a temporary file in the same directory, `fsync`, rename, then a directory `fsync`, all before the admission transaction commits. An existing blob is reused.
- **Other Sessions:** a fork (`runtime/fork.rs`), an import and a child task whose `attachments` carry references hard-link each referenced blob into the target Session's directory, falling back to a copy, before the target's events commit. Branches live in the same Session and share its blobs.
- **Deletion:** `owned_artifacts` adds the Session's attachment directory, so deletion and retention remove it with the Session.
- **Orphans:** a blob written for a prompt that was then refused, for a denied hook or a modality error, is removed by the refusal path. A crash in between leaves at most that blob until the Session is deleted, which is accepted.
- **In-memory stores:** with `durability: ephemeral` (`--ephemeral`), blobs go to a private temporary directory that is removed with the server.

Per-Session directories, rather than one global store, follow the browser-evidence pattern. Deleting a Session never has to check references held by other Sessions, and a reference can only resolve inside the Session that owns it. That is also why an `attachment` input part is safe: a client cannot reach another Session's blobs by guessing a hash.

### `file` parts and permissions

`file` parts are resolved in `Runtime::prompt_resources` next to template expansion, through a new `ToolHost::read_attachment(turn, path, asker, cancel)` (`cyber-tools/src/host/attachments.rs`). It builds the same `Invocation` as `read_reference` and runs `fs::readable`:

1. `external_directory` for paths outside the Location and the added directories,
2. the `read` rule,
3. `PreToolUse` hooks,
4. the hidden-path sandbox check,
5. the checkout's trust, which decides which project rules apply.

An `ask` reaches the attached client like any permission request. Under `plan` or `dont-ask` it becomes a denial. A denial refuses the whole prompt with `PermissionDeniedError` and the rule or mode, so no part of the prompt is admitted. The bytes then pass through the limits and normalization above.

`file` parts are accepted only on the local API. `http/peer/attach.rs` refuses them, because a paired Device's path means nothing on this machine and would read this machine's files. It accepts `attachment` parts: they name a blob the attached Session already stores and resolve only inside it, which is what an attached TUI sends when it resubmits a queued input taken back with its images. MCP-serve prompts build text parts only.

### Modality check and projection

- **At admission:** after resolution, the runtime looks up the Session's current model in the catalog. An image without `capabilities.vision`, or a PDF without `capabilities.pdf`, fails with `UnsupportedModalityError` 422 and the message `<provider>/<model> has no image input; switch models with /model or remove the attachment`. Nothing is admitted.
- **At Turn assembly:** after `view::messages`, `attachments::project` replaces each `Attachment` with `Image` or `Document` read from the store, cached for the Drain; the media type comes from the blob's magic bytes, never from the reference. It uses the `tool-registry` placeholder `[<mime> omitted: model has no <modality> input]` when the Turn's model lacks the input (a later `/model` switch or a fallback model), and `[<mime> <name> missing from storage]` when the blob is gone. Either way the model sees text, never a silent gap.
- **Compaction:** `strip` and `render_parts` cover `Attachment` and `Document` and produce `[Attached <mime>: <name>]`, as `compaction` → Media stripping on overflow requires.

### Provider mapping (`provider-catalog` → Image and PDF input)

| adapter | image | PDF |
|---|---|---|
| `anthropic` | `{type:"image",source:{type:"base64",…}}` (exists) | `{type:"document",source:{type:"base64",media_type:"application/pdf",data}}` |
| `openai_responses`, `chatgpt` | `input_image` with data URL (exists) | `{type:"input_file",filename,file_data:<data URL>}` |
| `openai_chat` | `image_url` (exists) | `{type:"file",file:{filename,file_data:<data URL>}}` |
| `copilot` | as its route, plus `Copilot-Vision-Request: true` (exists) | refused at admission (the Copilot catalog sets no `pdf`) |

Local OpenAI-compatible servers such as Ollama use `openai_chat`. Their models are vision-capable only when the catalog or `providers.<id>.models.<id>.capabilities.vision` says so, and the modality error names that key.

### Export and import

- **JSON:** the export document gains a top-level `attachments` array: `{ sha256, media_type, size, name, data }` for each referenced blob. Parts keep only the references.
- **Sanitize:** `--sanitize` omits the array and redacts each `name` as a `path` marker. `media_type` and `size` are kept, so the export says what was attached without carrying it.
- **Import:** each blob is checked against its `sha256` and `attachments.max_bytes` before it is written into the new Session's directory. A mismatch fails the import before any row is inserted.
- **HTML and Markdown:** the renderers in `cyber-client/src/export` show `[image: image/png, 1.2 MiB, shot.png]` (and `[pdf: …]`). The HTML page stays without images, as `add-html-session-export` decided.

### TUI

Most of this lives in the new `cyber-tui/src/attachments.rs`, so `app.rs` and `composer.rs` change little.

**Chips:**

- The composer keeps a chip table `N → payload` beside its text. A chip is a label in the text: `[Pasted text #N +L lines]`, `[Image #N]` or `[PDF #N]`.
- Backspace or Delete next to a label removes the whole label and its payload. A label the user edits by hand stops being a chip and is sent as typed.
- Numbering is per draft and never reuses a number.

**Paste:** `App::on_paste` (bracketed paste) checks, in order:

1. Dropped paths. The text is one or more paths separated by whitespace, each either single- or double-quoted, backslash-escaped (`My\ Shot.png`) or a `file://` URL. If every path is an existing regular file with an image or PDF extension, each becomes a chip.
2. Large text. Over 1,000 characters or 25 lines, unless `tui.paste_summary` is `false`, the paste becomes a text chip.
3. Otherwise the paste is inserted as text.

**Clipboard:**

- `composer.paste_image` (Ctrl+V; Alt+V on Windows, where Windows Terminal takes Ctrl+V for text paste) calls a `Clipboard` trait: `fn image(&self) -> Result<Option<Vec<u8>>, ClipboardError>`. The fake used in tests returns fixed bytes.
- The platform readers run fixed argument vectors through `std::process::Command`, never a shell string, and no user data reaches them:
  - **macOS:** `/usr/bin/osascript -e 'the clipboard as «class PNGf»'`, which prints the hex `«data PNGf…»` that the reader decodes.
  - **Linux:** `wl-paste --type image/png` when `WAYLAND_DISPLAY` is set, else `xclip -selection clipboard -t image/png -o`.
  - **Windows:** `powershell.exe -NoProfile -NonInteractive -Command <fixed script>`. The script reads `System.Windows.Forms.Clipboard.GetImage()`, saves it as PNG to a `MemoryStream` and prints base64. `Get-Clipboard -Format Image` exists only in Windows PowerShell 5.1, so it is not used.
- Each reader has a 5-second timeout and stops reading at `max_bytes + 1`.
- The toasts are `No image on the clipboard`, `Clipboard image needs wl-paste or xclip`, and `Attachment exceeds 20 MiB`.
- The reader is picked at run time with `cfg!`, so every platform compiles every argument vector and the unit tests check all of them. The only compile-time gate is the `0600` mode of the saved clipboard file on Unix.

**`@path`:** accepting an image or PDF from the `@` completion menu inserts a chip in place of the mention. On submit, a literal `@path` token with an image or PDF extension also becomes a chip's part. Text mentions keep today's behavior.

**Submit (`perform::parts`):**

- Text chips are replaced by their content inside the text part.
- Media chips stay as their labels in the text, so the model can tell which is which, and each adds one part in order:
  - `file { path }` for a path chip when the TUI talks to the server on its own machine,
  - inline `image` or `document` bytes for clipboard chips, and for path chips when the TUI is attached to a peer (`cyber attach`), where the server cannot see the TUI's files,
  - `attachment { sha256 }` for chips restored by rewind.
- **Size check:** the toast check runs in the client first (file metadata or clipboard length) so the user hears about it immediately. The server's check is the one that counts.
- **Errors:** a refused prompt (413, 415, 422 or a denial) keeps the draft and its chips and shows the server's message as a toast.

**Drafts:** text chips and path chips are saved with the draft. A clipboard image chip is saved with the draft too, as a private (`0600`) file in the TUI state directory that is removed on submit or clear. When the file is missing on restore, the toast `Clipboard image from the saved draft is gone` is shown and the chip is dropped.

**Transcript:** `model.rs` renders `[Image: <name>]` or `[PDF: <name>]` with the size (`[Image]` when there is no name).

### `cyber exec --file`

`exec/setup.rs::file_part` sniffs the first bytes of the file:

- **Image or PDF:** against a local server it sends `file { path }` with the absolute path. Exec runs in `dont-ask` by default, so a path outside the Location is denied unless `--add-dir` covers it. The run exits with code 2 and prints the server's `PermissionDeniedError` message with `Hint: add --add-dir <dir>`, naming the directory of the first image or PDF outside the Session's Location. With `--attach <url>` the CLI sends the bytes inline.
- **Text:** unchanged.

Refusals map to exit code 2 with the server's message: `AttachmentTooLargeError`, `UnsupportedAttachmentError`, `UnsupportedModalityError` and `PermissionDeniedError`. The 10 MiB data-URL rule in `exec-mode` is replaced by the server's `attachments.max_bytes`.

### OpenAPI and SDK

`PromptPart`, `AttachmentRef` and the three error schemas go into the OpenAPI document, regenerated with `UPDATE_OPENAPI=1`. The TS SDK is regenerated with `scripts/generate_sdk.py`. `session.prompt` already takes a `parts` array, so no hand-written helper is added.

## Risks / Trade-offs

- [The 64 MiB body limit on the prompt route] → It applies to that route only. Clients authenticate before the body is read, and the per-attachment and per-prompt caps bound memory after decoding.
- [`image` and `lopdf` add build time and attack surface] → Pure-Rust decoders with the PNG, JPEG, GIF and WebP features only. Decode limits are set before decoding, and decoding runs in `spawn_blocking` on bytes already capped at 20 MiB.
- [Clipboard tools are missing or differ between systems] → Each reader's failure becomes a toast that names the missing tool. Text paste is never affected.
- [Asks during admission keep the HTTP request open] → This is the same path command templates use today, and it is bounded by the permission request lifecycle.
- [A model switched after admission loses the image] → The placeholder says so in the model's input. The user can switch back, and the blob is still stored.
- [Exports get larger] → JSON exports embed the bytes, which is the only way import can round-trip. `--sanitize` drops them.
