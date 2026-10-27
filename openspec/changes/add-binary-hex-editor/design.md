## Baseline

The source baseline is `d36388d6`, the merge of PR #53. `cyber-core/src/files.rs` classifies a preview as binary from a bounded probe or failed text conversion and returns no byte content. `cyber-server/src/http/files.rs` serves project-scoped listing/search/text preview, revalidating file identity after permission waits. `cyber-tui/src/explorer.rs::Preview` has text lines or a binary size and no document mutation state. Model text edits and image/PDF prompt attachments are separate existing facilities.

Keep those interfaces compatible. Add a byte-document service shared by human editing and the model `binary` tool; do not make the old text `read`/`edit` tools reinterpret offsets or encode binary content as Unicode.

## Product experience

The normal entry is **Open in Hex** on a binary preview, or an explicit palette action on any regular file. `x` is the proposed explorer shortcut, subject to the existing keybinding collision checks; `/hex <path>` is the stable in-session entry. `cyber hex <path> [--offset <decimal|0xhex>] [--read-only] [--session <id>]` starts/reuses the ordinary owner server and opens the same workspace without starting inference. Resolve CLI paths against `--cwd`, then map them into the selected project root. A reused Session cannot be silently rebound to another Location; outside-project targets require an explicit appropriate `--cwd`/Session selection.

The first screen is **VIEW**, so navigation cannot alter bytes. The header shows path, size, observed revision, READ ONLY/VIEW/EDIT/REVIEW/SAVING/CONFLICT state and dirty-byte/range counts. Hex and ASCII cells share one cursor/selection. Show 16 bytes per row, with 8-byte grouping, at comfortable widths; use 8 or 4-byte rows and a drawer inspector on narrower terminals. Keep offsets readable and avoid horizontal page movement for ordinary navigation. Large files use virtual pages, never a giant text widget or an eager full-file decode.

The optional inspector shows unsigned/signed 8/16/32/64-bit values, float32/64 bit interpretations, bit layout and explicit LE/BE selection for the cursor range. Insufficient bytes show unavailable values; inspector interpretation never modifies content or implies file-format validity. A bounded known-signature label is informational only, never execution or an automatic parser. The change rail marks staged ranges; a status line shows cursor/selection offset, length, dirty state and current operation. See [ux.md](ux.md) for the visual and interaction contract.

Search accepts exact hex bytes or literal UTF-8 input; show the chosen encoding. Search patterns are capped at 4 KiB after encoding. No regex, wildcard, decompression or arbitrary parsing is implied. Goto supports decimal, `0x` hexadecimal and signed relative offsets with overflow/EOF checks. Bookmarks are document-revision-scoped, bounded to 64 named entries and kept out of model context unless the user explicitly shares them. Copy actions produce explicit hex, escaped text or offset references through the existing clipboard mechanism; arbitrary bytes never become terminal controls or shell input.

## Byte document and resource model

The server owns `document_id`, canonical project-relative path, Session/Location binding, native file identity and exact byte length immediately. A base SHA-256, draft revision and immutable base reference become available only after edit preparation; pending values are explicit rather than invented. Wire offsets/lengths are canonical decimal strings, with hex accepted at user-input boundaries, so SDK/JSON never loses values above JavaScript's safe integer range. Live VIEW pages carry observed-generation metadata and page fingerprints, not a claim that an arbitrarily changing whole file is immutable. Search over a live source is labelled live/best-effort and invalidates on observed changes; reproducible revision-consistent search uses a prepared immutable base or a proven native snapshot. A model patch requires the prepared exact base digest, never just a size/mtime token. Treat content as bytes throughout. The ASCII lane displays only printable ASCII 0x20–0x7e and a stable placeholder for other bytes; bytes are never repaired through a lossy string conversion.

Proposed first-release bounds:

| Resource | Default/maximum |
|---|---|
| Owner page | 64 KiB default, 1 MiB maximum; UI renders only visible rows |
| Model read result | 256 bytes default, 4 KiB maximum plus ordinary output budget |
| Search slice | At most 8 MiB or 250 ms of scanning per request, 100 results; revision-bound continuation |
| Patch batch | At most 128 nonoverlapping splices and 64 KiB combined expected/inserted bytes; 1 MiB encoded request cap |
| Editable file | At most 512 MiB initial and resulting size; larger regular files remain paged view/search only |
| Draft working storage | 2 GiB per document, including durable base/staging/recovery objects; reserve capacity before mutation |
| History | 1,000 local draft commands within the same storage cap; report a checkpoint before discarding older undo states |
| Clean abandoned drafts | 24-hour cleanup; dirty drafts and unresolved saves require explicit disposition and are not silently expired |

Limit open documents to four per Session and sixteen per server, with at most four editing preparations/saves active per server and admission checked against real available storage. These are proposed product limits for review, not measured performance claims. Do not add project settings that can silently widen server resource or authority ceilings. Profile navigation/search on a named reference machine with files larger than RAM and a 16 MiB editing fixture; record resident memory, first-page latency, input responsiveness and cancellation latency. First-page viewing must not wait for a full-file hash/copy. Exact-base preparation for editing can run in a cancellable background operation with honest progress; editing becomes available only when a bounded private immutable base has been established. Refuse mutation when secure storage, capacity or a native guarantee is unavailable, while preserving read-only access if permitted.

## Editing and save semantics

Use overwrite as the default edit mode. Entering edit mode, inserting/deleting bytes and pasting encoded data are explicit actions; neither ASCII focus nor a single hex nibble writes a file. A half-entered byte remains a UI input state until completed or cancelled. Changes are draft splices `{ offset, delete_length, expected_hex, insert_hex }` against a named base/draft revision. Each expected byte range and all arithmetic are checked; reject overlap, ambiguous same-offset insertions, out-of-range spans, odd/invalid hex and revision mismatches before changing any draft. The expected bytes cover the deleted/replaced region; insertion also relies on the exact base revision. Multi-splice batches use original-base coordinates and commit as one draft command. Local undo/redo operates on the draft only and is distinct from Session `/undo`/`/redo` and post-save file restore.

Review shows before/after hex with stable original offsets, inserted/deleted counts, final length, base/proposed hashes and excluded/nonreversible effects. Byte changes are summarized as ranges, not a fabricated text diff. Saving requires the current read/edit permission decisions, exact review/draft revision, stable operation ID and expected base. `read` is necessary for any before/after disclosure; an edit allowance does not grant a denied read. Protected paths, readonly sandbox and Mode ceilings remain effective for UI, API and model operations. Ordinary approval rules determine whether an exact model patch requires an ask; binary content cannot approve itself.

Before modifying the target, reserve capacity for the immutable preimage and replacement, obtain the existing workspace mutation lease and revalidate path/file identity and current bytes. Build the replacement by bounded streaming into a private regular temporary file on the required filesystem, preserving supported permissions/executable metadata. Use a native-reviewed atomic replacement with recoverable displaced preimage and a durable intent/outcome journal; never truncate the live file in place. Verify the displaced version and final installed content before acknowledging success. Reconcile restore/lease/version behavior with the Daily coding snapshot work.

Cyber's lease coordinates Cyber writers, not arbitrary external editors. Do not claim a portable atomic compare-and-swap against an uncooperative process that can keep writing an already-open inode. Native race tests must establish the precise guarantee: detect pre-save conflicts, preserve displaced bytes for a detected replacement race, and retain an unknown/conflict outcome if the journal cannot establish what was installed. Never report unconditional success or retry a possibly completed write just because an acknowledgement was lost. Unsupported replacement semantics refuse editing until a reviewed implementation exists. Symlink, hardlink, path-parent replacement and read-only-file handling are explicit native tests; initially refuse symlink/multiply linked mutation targets rather than changing an unexpected alias.

Save retry returns the existing receipt for the same operation and canonical payload. Reusing an operation ID for different bytes is a conflict. Restart retains dirty draft/journal evidence; resuming requires fresh owner authentication, Session/Location and permissions, and base reconciliation. Path or Location changes do not move a draft's authority. Restore Last Save is a separately reviewed conflict-aware file restore using the recorded preimage, not an unguarded overwrite. Successful saves feed the shared snapshot/diff and committed FileChanged observations once; uncertain saves do not fabricate success events. Git and non-git Locations require equivalent binary preimage evidence even when the ordinary snapshot's untracked-file limit would skip the file. External format checksums, signatures and remote effects are never repaired automatically.

## Owner API and model integration

Add typed owner-only routes to `fs`:

- `POST /api/v1/files/binary/documents`: open an authorized regular file in view/edit intent and return metadata/base-preparation status.
- `GET /api/v1/files/binary/documents/{id}/bytes`: bounded revision-scoped byte page, encoded as base64 with explicit offset/length; raw file bytes never enter URLs.
- `POST .../{id}/search`: literal byte/text search with bounded revision-scoped continuation.
- `POST .../{id}/patches`: stage one expected-revision batch with idempotent operation identity.
- `POST .../{id}/history`: explicitly undo/redo a draft command; no direct file mutation.
- `POST .../{id}/review`: produce a stable byte-diff/save review tied to the exact draft.
- `POST .../{id}/save`: submit the reviewed revision/operation with expected base and receive acknowledged/conflict/unknown status.
- `POST .../{id}/restore`: review/apply the retained last-save preimage through the same identity/conflict protocol.
- `GET .../{id}/operations/{operation_id}`: recover a lost acknowledgement.
- `DELETE .../{id}`: explicitly close/discard a draft; unresolved mutation evidence is not discarded.

File I/O, hashing and encoding use bounded workers rather than blocking the server event loop. All document routes retain owner authentication, Origin rules, private caching, regular-file admission and the selected Session/Location resource authority. Opaque IDs are not bearer capabilities. Preserve `/files/content` response compatibility and existing browser static/content constraints. The TypeScript SDK exposes byte arrays at its convenience boundary but keeps offsets exact; it uses the same identity/revision types and operation lookup. The CLI is an owner client, not a local filesystem shortcut.

Register one capability-owned model tool, `binary`, with `inspect`, `read`, `search` and `patch` operations. Read operations check ordinary `read`; patch additionally checks `edit`, code-restore protections and invocation-scoped write authority. Plan/read-only agents can inspect/search but cannot patch. The model receives bounded offset/hex/ASCII or match results, base fingerprints and explicit receipts; it cannot send a complete arbitrary file as unbounded base64 or invoke the owner's unsaved draft. A model patch owns a separate ephemeral draft, validates all expected bytes, passes ordinary permission review with its byte diff, then uses the same save engine and durable operation identity. No new provider request is required for manual editing. Hook/plugin code cannot bypass its declared filesystem capabilities by calling this API/tool.

## Delivery and acceptance

Implement after reviewed foundations: byte-service native identity/storage proof; bounded view/search and SDK; staged editing/save/recovery; themed TUI/direct CLI; model patch integration; snapshot/hooks and acceptance. Tests must cover every byte value, empty files, NUL/invalid UTF-8, offsets above 2^53, sparse files, EOF/overflow, same-size/same-mtime changes, denied reads/edits, partial nibbles, multi-splice atomicity, file growth/shrinkage, history, stale revisions and Session switches, lost acknowledgements, kill/restart around replacement and native alias races. Use real binary fixtures and hashes, TUI snapshots at wide/narrow/NO_COLOR widths plus real terminal captures, native platform positive/negative controls, generated schema/API/CLI checks and code/security review. Baseline tests have not been run for this planning-only extension.
