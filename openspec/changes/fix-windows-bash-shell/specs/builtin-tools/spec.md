## MODIFIED Requirements

### Requirement: bash tool
(P0) `bash` SHALL accept `{ command, timeout_ms? (default 120000, max 600000), workdir?, description?, background? }`. It SHALL run in the configured `shell`, else `$SHELL` when acceptable (bash, zsh, sh, dash), else `/bin/sh`. On Windows, a configured shell that is not an existing absolute path to a supported shell SHALL be resolved by its name (anything unsupported as `bash`) from `PATH` and Git for Windows installs, never from the WSL launcher directories (`System32`, `Sysnative`, `WindowsApps`) or from inside the checkout; an explicitly configured absolute path SHALL be used as given; when no shell is found the call SHALL fail with `No POSIX shell found: install Git for Windows or set "shell" to the absolute path of bash.exe`. Stdin SHALL be closed and stdout/stderr combined. The environment SHALL carry `CYBER=1`, `CYBER_SESSION_ID` and `CYBER_PID`. The tool SHALL run inside the sandbox per the `sandbox` capability. Capture SHALL be limited to 1 MiB, keeping the tail, with the full output in a managed file. The exit code SHALL be reported as `exit_code` metadata, and empty output SHALL be returned as `(no output)`. `background: true` SHALL start a background task and return its `job_` ID immediately.

#### Scenario: Timeout
- **WHEN** a command exceeds `timeout_ms`
- **THEN** its process tree is killed and the result ends with `Command timed out after <n> ms`

#### Scenario: Background command
- **WHEN** the model calls `bash` with `background: true` for `npm run dev`
- **THEN** the call returns `job_<id>` immediately and the output streams into the background task

#### Scenario: Windows never picks the WSL launcher by name
- **WHEN** the server runs on Windows with Git for Windows installed, `shell` is unset and `C:\Windows\System32\bash.exe` exists
- **THEN** `bash` runs Git's `bash.exe`/`sh.exe` by absolute path, not the WSL launcher
