## MODIFIED Requirements

### Requirement: Attachment normalization
(P0) Prompt attachments (TUI paste or drop, `@path` mentions, `--file`, remote uploads) SHALL obey the `attachments` config:

- `max_bytes`: default 20 MiB per file. Larger files are rejected with `AttachmentTooLargeError`, and a file part is checked before it is read.
- `image.max_dimension`: default 2000 px. Larger images are downscaled, preserving the aspect ratio.
- `image.max_bytes`: default 5 MiB. Images above it are re-encoded as JPEG quality 85.
- `pdf.max_pages`: default 100. Longer PDFs are attached with a page-range prompt.

The media type SHALL be taken from the bytes, never from the client's `media_type` or the file extension. Only PNG, JPEG, GIF and WebP images and PDF documents SHALL be accepted, and anything else SHALL be rejected with `UnsupportedAttachmentError`. Images SHALL be decoded under limits of 100 megapixels and 512 MiB of allocation, and an image over them SHALL be rejected before its pixels are decoded. Normalization SHALL be deterministic.

Normalized attachments SHALL be stored as managed artifacts (`storage-events` → Attachment storage) referenced from the message part. The original SHALL be kept when `attachments.keep_original` is true (default false).

The Session's model is resolved at admission:

- A prompt with an image SHALL be refused with `UnsupportedModalityError` when the model lacks `capabilities.vision`, and one with a PDF when it lacks `capabilities.pdf`. The message SHALL be `<provider>/<model> has no <image|pdf> input; switch models with /model or remove the attachment`, and nothing SHALL be admitted.
- When a later Turn runs on a model without the input, a model switch or a fallback, earlier attachments SHALL reach that model as the placeholder defined by `tool-registry`.
- An attachment whose blob is missing SHALL reach the model as `[<mime> <name> missing from storage]`.

Attachments SHALL never be dropped without a placeholder.

#### Scenario: Large screenshot downscaled
- **WHEN** the user pastes a 4000×3000 PNG of 9 MiB
- **THEN** the stored attachment is at most 2000 px on its longest side and under 5 MiB, and the model receives the downscaled image

#### Scenario: Extension does not decide the type
- **WHEN** a client attaches a ZIP archive renamed to `photo.png`
- **THEN** the prompt is refused with `UnsupportedAttachmentError` and nothing is stored

#### Scenario: Text-only model refuses an image
- **WHEN** the Session's model has `capabilities.vision` false and a prompt carries an image
- **THEN** the prompt is refused with `UnsupportedModalityError` naming the model, and no Turn starts

#### Scenario: Model switched after an image
- **WHEN** a Session admitted an image on a vision model and then switches to a text-only model
- **THEN** the next Turn's request carries `[image/png omitted: model has no image input]` in place of the image

#### Scenario: Long PDF
- **WHEN** the user attaches a 240-page PDF
- **THEN** the model receives the PDF and the note that only pages 1-100 are expected to be read unless the user names others
