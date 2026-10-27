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
