## MODIFIED Requirements

### Requirement: notebook_edit tool
(P1) `notebook_edit` SHALL accept `{ path, cell_id?, cell_index?, new_source, cell_type?, mode: replace|insert|delete }` for `.ipynb` files. It SHALL check the `edit` permission, preserve notebook metadata and outputs of untouched cells, and clear the outputs of edited code cells. Cell indices SHALL be zero-based. Both selectors, when supplied, SHALL identify the same cell. Replace/delete SHALL require a selector; insert without a selector SHALL append. Insertion at an index or cell ID SHALL occur before that cell. Edited code execution counts SHALL become null. Untouched cell IDs, source representation, attachments, outputs and extension fields SHALL remain unchanged. Opaque metadata and untouched outputs SHALL preserve exact numeric literals without floating-point conversion.

No-op replacement SHALL still check edit permission and SHALL preserve original file bytes. Mutations and no-ops SHALL revalidate file bytes under the host path lock after permission approval and refuse intervening user edits. Modern notebooks SHALL validate unique cell IDs and receive a fresh ID for an inserted cell; legacy notebook versions SHALL remain unchanged. The tool SHALL neither run notebook code nor start a kernel. Plan-mode catalog admission SHALL exclude it, and both ordinary and apply-patch-preferring model catalogs SHALL offer it.

#### Scenario: Insert markdown cell
- **WHEN** `mode` is `insert` with `cell_type: "markdown"` at index 0
- **THEN** a new markdown cell becomes the first cell and the other cells are unchanged

#### Scenario: Replace a code cell by ID
- **WHEN** a code cell is replaced using its unique ID
- **THEN** its metadata and ID SHALL remain unchanged while outputs are cleared and execution_count becomes null
- **AND** all other notebook fields and cells SHALL remain semantically unchanged

#### Scenario: File changes during approval
- **WHEN** a user changes notebook bytes while an edit or no-op request is awaiting permission
- **THEN** the request SHALL refuse after approval and preserve those user changes

#### Scenario: No-op is denied
- **WHEN** an unchanged replacement is requested while edit permission is denied
- **THEN** the tool SHALL refuse without rewriting the notebook

#### Scenario: Conflicting selectors
- **WHEN** cell_id and cell_index identify different cells
- **THEN** the request SHALL fail without changing notebook bytes


#### Scenario: Opaque numeric metadata
- **WHEN** notebook metadata, target-cell metadata or untouched outputs contain large numeric literals
- **THEN** editing a cell SHALL preserve those literals exactly, including values outside floating-point range

### Requirement: bash tool
(P0) `bash` SHALL accept `{ command, timeout_ms? (default 120000, max 600000), workdir?, description?, background?, ready_url?, ready_timeout_seconds? }`. It SHALL run in the configured `shell`, else `$SHELL` when acceptable (bash, zsh, sh, dash), else `/bin/sh`. Stdin SHALL be closed and stdout/stderr combined. The environment SHALL carry `CYBER=1`, `CYBER_SESSION_ID` and `CYBER_PID`. The tool SHALL run inside the sandbox per the `sandbox` capability. Capture SHALL be limited to 1 MiB, keeping the tail, with the full output in a managed file. The exit code SHALL be reported as `exit_code` metadata, and empty output SHALL be returned as `(no output)`. `background: true` SHALL start a background task and return its `job_` ID immediately.

`ready_url` (P1, `browser-verification` → Verification artifacts and lifecycle) SHALL apply only with `background: true` and SHALL be an `http(s)` URL on this machine (`localhost`, a loopback address). The call SHALL then return only once that URL answers with an HTTP status below 500. When the process exits first, or `ready_timeout_seconds` (default 60, at most 600) passes, the Job SHALL be stopped. The call SHALL then fail with the log tail and record `startup_failed` browser evidence stating that the page was not tested. A call cancelled while it waits SHALL stop the Job. The readiness check SHALL NOT follow redirects. Where the sandbox isolates the command's loopback from the host (a sandboxed command on Windows, or on Linux and macOS below a `sandbox.network` other than `on`), `ready_url` SHALL be refused before the command starts. `powershell` SHALL accept the same two parameters.

#### Scenario: Timeout
- **WHEN** a command exceeds `timeout_ms`
- **THEN** its process tree is killed and the result ends with `Command timed out after <n> ms`

#### Scenario: Background command
- **WHEN** the model calls `bash` with `background: true` for `npm run dev`
- **THEN** the call returns `job_<id>` immediately and the output streams into the background task

#### Scenario: Dev server readiness
- **WHEN** the model calls `bash` with `background: true` and `ready_url: "http://127.0.0.1:5173/"`, and the server exits with code 1 before it listens
- **THEN** the call fails with `Dev server startup failed`, the exit code and the log tail, and `startup_failed` evidence is recorded for the Session
