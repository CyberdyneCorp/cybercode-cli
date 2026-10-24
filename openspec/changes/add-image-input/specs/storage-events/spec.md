## ADDED Requirements

### Requirement: Attachment storage
(P0) Prompt attachments SHALL be stored once, as immutable content-addressed blobs at `<data>/attachments/<session_id>/<sha256>`, and events SHALL hold only `attachment { sha256, media_type, name, size }` references.

- **Durability:** each blob SHALL be written to a temporary file, synced, renamed and its directory synced before the transaction that references it commits.
- **Reuse and refusal:** an existing blob SHALL be reused. A blob written for a prompt that is then refused SHALL be removed.
- **Copies:** a fork, an import or a child task that carries references SHALL link or copy the referenced blobs into the target Session's directory before the target's events commit.
- **Resolution:** a reference SHALL resolve only within its own Session.
- **Deletion:** deleting a Session, or removing it by retention, SHALL remove its attachment directory.
- **Ephemeral stores:** an in-memory store SHALL keep blobs in a private temporary directory removed with the server.

The JSON export (`cyber sessions export`) SHALL carry a top-level `attachments` array of `{ sha256, media_type, size, name, data }` for the referenced blobs.

- **Sanitize:** `--sanitize` SHALL omit the array and redact each attachment `name` as a `path` marker, while keeping `media_type` and `size`.
- **Markdown and HTML:** these exports SHALL show `[image: <media_type>, <size>, <name>]` or `[pdf: …]` placeholders, without the bytes.
- **Import:** `cyber sessions import` SHALL verify each blob's SHA-256 and `attachments.max_bytes` before inserting any row, and SHALL fail without changes on a mismatch.
  The document is untrusted. A message with inline `image` or `document` parts SHALL be refused, because media is imported only as verified blobs. A system message SHALL become user text marked as imported, so it never reaches the model as a system reminder.

Events written before this requirement, with inline base64 `image` parts, SHALL still replay unchanged.

#### Scenario: Replay without duplicated bytes
- **WHEN** a Session with an 8 MiB screenshot is replayed after a server restart
- **THEN** the event log holds the reference only, and the next Turn sends the same image read from the blob

#### Scenario: Sanitized export drops attachment bytes
- **WHEN** the user exports a Session with a screenshot with `--sanitize`
- **THEN** the document has no `attachments` array, the part shows `image/png` and its size, and its name is a `[redacted:path:<n>]` marker

#### Scenario: Export round-trip
- **WHEN** a Session with a PDF is exported as JSON and imported on another machine
- **THEN** the imported Session's next Turn sends the same PDF

#### Scenario: Tampered import
- **WHEN** an export's attachment `data` does not match its `sha256`
- **THEN** the import fails and no Session is created

#### Scenario: Crafted import
- **WHEN** an export carries an inline image part, or a system message
- **THEN** the inline image is refused and no Session is created, and the system message reaches the model only as marked user text

#### Scenario: Deletion removes blobs
- **WHEN** a Session with attachments is deleted
- **THEN** its `<data>/attachments/<session_id>/` directory no longer exists, and a fork made earlier still resolves its own copies
