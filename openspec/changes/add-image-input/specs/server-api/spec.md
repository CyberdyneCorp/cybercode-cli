## ADDED Requirements

### Requirement: Prompt attachment parts
(P0) `POST /api/v1/sessions/:sessionID/prompt` SHALL accept `parts` of these types:

- `text { text }`
- `image { media_type, data, name? }` and `document { media_type, data, name? }`, where `data` is base64
- `file { path, name? }`, read by the server
- `attachment { sha256 }`, naming a blob this Session already stores

Any other part type SHALL be refused with `InvalidRequestError`. The route SHALL accept request bodies up to 64 MiB. Each prompt SHALL carry at most 20 media parts.

Media handling:

- **Before admission:** every media part SHALL be decoded, sniffed, size-checked, normalized and stored as `session-runtime` → Attachment normalization and `storage-events` → Attachment storage define. The admitted message SHALL hold `attachment { sha256, media_type, name, size }` references only.
- **`file` parts:** read through the `read` tool's checks (the agent profile's tools and a loaded skill's tool limits, `external_directory`, the `read` rule, hooks, hidden paths), with asks reaching the attached client. They SHALL be refused on the peer listener.
- **`attachment` parts:** a `sha256` that this Session does not store SHALL be refused with `InvalidRequestError`.
- **Message history:** `GET .../messages` SHALL return the references, not the bytes.
- **Digest:** the admission digest SHALL replace inline `data` with its SHA-256, so a retried request with the same bytes replays the same receipt.

The OpenAPI document SHALL describe the part union and `AttachmentRef`.

#### Scenario: Inline screenshot stored once
- **WHEN** a client posts a prompt with an 8 MiB base64 PNG `image` part
- **THEN** the prompt is admitted, the stored event holds an `attachment` reference without base64 data, and the model receives the image

#### Scenario: Unknown part type
- **WHEN** a client posts a prompt whose parts contain `{ "type": "tool_call", … }`
- **THEN** the response is `400` `InvalidRequestError` and nothing is admitted

#### Scenario: File part denied
- **WHEN** an exec Session in `dont-ask` mode receives `file { path: "/etc/shadow" }`
- **THEN** the response is a permission denial naming the rule or mode, and nothing is read or admitted

#### Scenario: Peer cannot send paths
- **WHEN** a paired Device posts a prompt with a `file` part through the peer listener
- **THEN** the request is refused with `InvalidRequestError` and no file is read

#### Scenario: Peer resubmits a stored blob
- **WHEN** a paired Device posts a prompt with an `attachment` part naming a blob the Session stores (a queued input taken back in `cyber attach`)
- **THEN** the part passes the peer listener and is handled as on the local API

### Requirement: Media on child and inbox edit routes
(P0) A subtask's or delegation's `attachments` and an edited queued input's `parts` (`PATCH .../inbox/:messageID`) SHALL take the prompt part union with the same handling, except that `file` parts SHALL be refused. These routes SHALL accept bodies up to 64 MiB. Their media SHALL suit the model of the Session that admits them (the child, or the edited Session), and a refused request SHALL remove the blobs it wrote.

#### Scenario: Delegated PDF stored by reference
- **WHEN** `cyber exec "@explore read it" --file spec.pdf` delegates to a child whose model takes PDFs
- **THEN** the child's events hold an `attachment` reference without base64 data; on a model without PDF input the request is refused and nothing is left behind

#### Scenario: Edited input with a mislabelled reference
- **WHEN** a client edits a queued input with `attachment { sha256, media_type: "application/pdf" }` naming a stored PNG
- **THEN** the edited input holds an `image/png` reference rebuilt from the blob

## MODIFIED Requirements

### Requirement: Tagged error model
(P0) Declared failures SHALL be JSON objects tagged by `_tag`, with these statuses: `InvalidRequestError` 400, `InvalidCursorError` 400, `UnauthorizedError` 401, `ForbiddenError` 403, `AccountRequiredError` 403, `EntitlementRequiredError` 402, `NotFoundError` variants 404 (`SessionNotFoundError`, `MessageNotFoundError`, `RunNotFoundError`, `AddressNotFoundError`, …), `ConflictError` 409, `SessionBusyError` 409, `AmbiguousAddressError` 409 with `candidates`, `MessageTooLargeError` 413 with `size` and `limit`, `AttachmentTooLargeError` 413 with `size` and `limit`, `UnsupportedAttachmentError` 415 with `media_type`, `ReviewTargetError` 422 with `code` (from `v1.command.run`, `vcs-integration` → Local review command), `UnsupportedModalityError` 422 with `model` and `modality` (`image` or `pdf`), `RateLimitedError` 429 with `retryAfterMs`, `UnknownError` 500 with a `ref` of the form `err_<id>` logged server-side, and `ServiceUnavailableError` 503.

#### Scenario: Unexpected failure
- **WHEN** a handler fails with an undeclared error
- **THEN** the response is `500` with `{ "_tag": "UnknownError", "message": "...", "ref": "err_..." }` and the same `ref` appears in the server log

#### Scenario: Ambiguous message address
- **WHEN** a client posts to `/api/v1/messaging/send` with `to: "migration"` and two Sessions have that name
- **THEN** the response is `409` with `_tag: "AmbiguousAddressError"` and `candidates` listing both Session IDs

#### Scenario: Oversized attachment
- **WHEN** a client posts a prompt with a 25 MiB PNG
- **THEN** the response is `413` with `_tag: "AttachmentTooLargeError"`, `limit` 20971520 and the message `Attachment exceeds 20 MiB`
