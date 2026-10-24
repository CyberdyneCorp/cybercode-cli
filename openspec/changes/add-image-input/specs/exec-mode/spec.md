## MODIFIED Requirements

### Requirement: Slash commands, files and attachments
(P0) The system SHALL accept `--command <name>`, which runs a custom command or skill with the assembled prompt as its arguments. `--file <path>` (repeatable) SHALL attach files resolved against the Location and fail with exit code 2 for missing paths. Directories SHALL be rejected. Text files SHALL be attached as text. Images (PNG, JPEG, GIF, WebP) and PDFs, detected from their bytes, SHALL be attached as attachment parts (`server-api` → Prompt attachment parts):

- **Local server:** as `file` parts, which the server reads under the `read` permission and the exec mode. A path outside the Location in `dont-ask` mode is denied unless `--add-dir` covers it.
- **`--attach <url>`:** as inline bytes read by the CLI.

Attachments SHALL obey the server's `attachments.max_bytes` (default 20 MiB). A refused attachment (too large, unsupported type, model without the input, or a permission denial) SHALL end the run with exit code 2 and the server's message, before any Turn runs.

#### Scenario: Run a custom command
- **WHEN** a user runs `cyber exec --command review "main..HEAD"`
- **THEN** the `review` command template is expanded with `$ARGUMENTS` = `main..HEAD` and executed

#### Scenario: Screenshot attached
- **WHEN** a user runs `cyber exec --file shot.png "what is wrong in this UI?"` with a vision model
- **THEN** the model receives the image and the run exits with code 0

#### Scenario: Text-only model
- **WHEN** the same command runs with a model that has no image input
- **THEN** the run exits with code 2 and stderr names the model and `has no image input`
