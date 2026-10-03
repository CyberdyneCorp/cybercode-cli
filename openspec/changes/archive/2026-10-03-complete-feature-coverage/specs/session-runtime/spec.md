## ADDED Requirements

### Requirement: Attachment normalization
(P0) Prompt attachments (TUI paste or drop, `@path` mentions, `--file`, remote uploads) SHALL obey the `attachments` config: `max_bytes` (default 20 MiB per file, larger files rejected with `AttachmentTooLargeError`), `image.max_dimension` (default 2000 px; larger images are downscaled preserving aspect ratio), `image.max_bytes` (default 5 MiB; images above it are re-encoded as JPEG quality 85) and `pdf.max_pages` (default 100; longer PDFs are attached with a page-range prompt). Normalized attachments SHALL be stored as managed artifacts referenced from the message part, and the original SHALL be kept when `attachments.keep_original` is true (default false). Models without the required input modality SHALL receive the placeholder defined by `tool-registry`.

#### Scenario: Large screenshot downscaled
- **WHEN** the user pastes a 4000×3000 PNG of 9 MiB
- **THEN** the stored attachment is at most 2000 px on its longest side and under 5 MiB, and the model receives the downscaled image
