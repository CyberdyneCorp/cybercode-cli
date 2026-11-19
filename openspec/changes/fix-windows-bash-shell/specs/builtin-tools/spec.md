## MODIFIED Requirements

### Requirement: bash tool
(P0) `bash` SHALL accept `{ command, timeout_ms? (default 120000, max 600000), workdir?, description?, background? }`. It SHALL run in the configured `shell`, else `$SHELL` when acceptable (bash, zsh, sh, dash), else `/bin/sh`. On Windows, a configured shell that is not an existing absolute path to a supported shell SHALL be resolved by its name (anything unsupported as `bash`) from `PATH` and Git for Windows installs, never from the WSL launcher directories (`System32`, `Sysnative`, `WindowsApps`) or from inside the checkout; an explicitly configured absolute path SHALL be used as given; when no shell is found the call SHALL fail with `No POSIX shell found: install Git for Windows or set "shell" to the absolute path of bash.exe`. Stdin SHALL be closed and stdout/stderr combined. The environment SHALL carry `CYBER=1`, `CYBER_SESSION_ID` and `CYBER_PID`. So that no command waits for a person at an editor, pager or credential prompt, every model-run process (`bash`, `powershell`, `monitor`, `pty_start`, background commands, worktree setup and solve checks) SHALL get `GIT_EDITOR`, `JJ_EDITOR`, `HGEDITOR`, `EDITOR` and `VISUAL` set to `true` (a no-op editor that leaves the message unchanged), `PAGER`, `GIT_PAGER`, `JJ_PAGER` and `MANPAGER` set to `cat`, and `GIT_TERMINAL_PROMPT=0`, replacing inherited values; a variable named in `sandbox.env.set` SHALL keep its configured value. The tool SHALL run inside the sandbox per the `sandbox` capability. Capture SHALL be limited to 1 MiB, keeping the tail, with the full output in a managed file. The exit code SHALL be reported as `exit_code` metadata, and empty output SHALL be returned as `(no output)`. `background: true` SHALL start a background task and return its `job_` ID immediately.

#### Scenario: Timeout
- **WHEN** a command exceeds `timeout_ms`
- **THEN** its process tree is killed and the result ends with `Command timed out after <n> ms`

#### Scenario: Commit without a message
- **WHEN** the model runs `git commit` without `-m` in a repository whose `core.editor` is an interactive editor
- **THEN** the no-op editor leaves the message empty and Git aborts at once instead of waiting until the timeout

#### Scenario: Pager on a terminal
- **WHEN** the model starts `git log` with `pty_start`
- **THEN** the log is printed through `cat` and the program ends without waiting for a keypress

#### Scenario: Configured editor kept
- **WHEN** `sandbox.env.set` sets `PAGER` to `more`
- **THEN** commands see `PAGER=more` and `GIT_PAGER=cat`

#### Scenario: Background command
- **WHEN** the model calls `bash` with `background: true` for `npm run dev`
- **THEN** the call returns `job_<id>` immediately and the output streams into the background task

#### Scenario: Windows never picks the WSL launcher by name
- **WHEN** the server runs on Windows with Git for Windows installed, `shell` is unset and `C:\Windows\System32\bash.exe` exists
- **THEN** `bash` runs Git's `bash.exe`/`sh.exe` by absolute path, not the WSL launcher
