## MODIFIED Requirements

### Requirement: Prompt attachment parts
(P0) `POST /api/v1/sessions/:sessionID/prompt` SHALL accept `parts` of these types:

- `text { text }`
- `image { media_type, data, name? }` and `document { media_type, data, name? }`, where `data` is base64
- `file { path, name?, range? }`, read by the server. (P3) `range` is `{ start_line, end_line }`, 1-based and inclusive, with `1 <= start_line <= end_line`; only those lines SHALL be admitted, after the same checks as the whole file. A range past the end of the file SHALL be clamped to the last line, and a malformed range SHALL be refused with `InvalidRequestError`.
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

#### Scenario: Ranged file part
- **WHEN** a local client posts `file { path: "src/app.ts", range: { start_line: 12, end_line: 30 } }`
- **THEN** the server applies the `read` tool's checks to `src/app.ts` and admits only lines 12 through 30

#### Scenario: Malformed range
- **WHEN** a client posts `file { path: "src/app.ts", range: { start_line: 30, end_line: 12 } }`
- **THEN** the response is `400` `InvalidRequestError` and nothing is read or admitted
