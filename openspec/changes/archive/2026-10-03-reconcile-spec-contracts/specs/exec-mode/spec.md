## MODIFIED Requirements

### Requirement: Execution backend and working directory
(P0) The system SHALL run against the registered background server by default, starting it as `cyber service start` does. `--embedded` SHALL run a private in-process server on a private database (in-memory when combined with `--ephemeral`) that never opens the shared database for writing. `--attach <url>` SHALL target a remote server. Remote authentication SHALL use `--password` or `CYBER_SERVER_PASSWORD`, or the Cyber Account token for Relay URLs. The global `--cwd <dir>` SHALL set the Location before execution and fail with exit code 2 when the directory does not exist locally, unless `--attach` is set, in which case the path is sent to the server as-is.

#### Scenario: Attach to a remote runner
- **WHEN** CI runs `cyber exec --attach https://runner.internal:4747 --cwd /work/repo "run lint"`
- **THEN** the Session is created on the remote server in `/work/repo` and events stream back to the CI log

#### Scenario: Default uses the background server
- **WHEN** a user runs `cyber exec "hi"` while the TUI has a Session running on the registered server
- **THEN** the prompt is admitted through that server and no second writer process is started

### Requirement: Ephemeral runs
(P0) The system SHALL accept `--ephemeral`, which implies `--embedded` and runs the Session in an in-memory store. Nothing SHALL be written to the shared database, snapshots or share service, and no resume hint SHALL be printed. `--ephemeral` combined with `--continue`, `--session` or `--fork` SHALL fail with exit code 2.

#### Scenario: Nothing persisted
- **WHEN** `cyber exec --ephemeral "summarize README"` completes
- **THEN** `cyber sessions list` shows no new Session

### Requirement: Output formats
(P0) The system SHALL honor the global `--format text|json|stream-json` (default `text` for `exec`):
- **text**: final assistant text on stdout; tool progress lines on stderr only when `--verbose` is set.
- **json**: one JSON object on stdout at the end, with `result`, `session_id`, `usage`, `cost_usd`, `duration_ms`, `num_turns`, `denials` and `exit_code`.
- **stream-json**: one JSON object per line as events occur.

`--quiet` SHALL suppress all non-result output on stderr.

#### Scenario: JSON result for scripting
- **WHEN** a user runs `cyber exec --format json "count TODOs"`
- **THEN** stdout contains exactly one JSON object whose `result` holds the final text and whose `exit_code` is 0

### Requirement: Run limits
(P0) The system SHALL accept the Budget flags defined in `observability-costs`:
- `--max-turns <n>`: on reaching the limit, the final Turn is sent without tools.
- `--max-tokens <n>` and `--max-cost <usd>`: checked after each Turn.
- `--timeout <duration>`, for example `30m`.

Exceeding `--max-tokens`, `--max-cost` or `--timeout` SHALL interrupt the Drain, emit a `result` with `stop_reason` `budget_exceeded` or `timeout`, and exit with code 4.

#### Scenario: Cost cap reached
- **WHEN** a run with `--max-cost 0.50` reaches $0.53 after a Turn
- **THEN** the Drain is interrupted, `result.stop_reason` is `budget_exceeded`, and the exit code is 4

### Requirement: CI usage
(P1) The system SHALL run headless in CI without a TTY, config files or account. It SHALL read provider credentials from environment variables and never open a browser. Output SHALL contain no ANSI color codes unless `--color always` is set. The resume hint SHALL be printed to stderr as `session: <id>` only when the Session is persisted.

#### Scenario: GitHub Actions step
- **WHEN** a workflow step runs `cyber exec --format stream-json --max-turns 30 "fix lint errors"` with `OPENAI_API_KEY` set
- **THEN** the run completes without prompts, logs JSONL to stdout, and the job fails only when the exit code is non-zero

#### Scenario: GitLab CI job
- **WHEN** a `.gitlab-ci.yml` job runs `cyber exec --mode accept-edits "update snapshots" && git diff --exit-code || true`
- **THEN** edits apply without approval prompts and no color codes appear in the job log
