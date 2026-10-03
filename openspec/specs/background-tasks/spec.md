# background-tasks Specification

## Purpose
Background tasks let long-running commands, monitors and interactive programs run without blocking the conversation, and let the agent reach the user when work finishes. Cyber Code combines Claude Code's background bash, `Monitor`, `/tasks`, push notifications and file delivery, Codex's `/ps` and `/stop`, and OpenCode's PTY service with replay buffers, all tracked in a durable job registry.

## Requirements

### Requirement: Background bash
(P1) The `bash` tool SHALL accept `run_in_background: true`, start the command as a Job (`job_` ID) and return immediately with the Job ID and the path of its output file under `<data>/jobs/<job_id>.log`. The Job SHALL keep running after the Turn ends, under the same sandbox and permissions as foreground commands.

#### Scenario: Start a dev server
- **WHEN** the model runs `npm run dev` with `run_in_background: true`
- **THEN** the tool returns at once with the Job ID and log path, and the conversation continues

### Requirement: Job registry
(P1) The system SHALL record each Job with `id`, `session_id`, `kind` (`bash`, `monitor`, `pty`, `subagent`, `workflow`), `command` or description, `status` (`running`, `completed`, `error`, `cancelled`, `interrupted`), `exit_code`, start and end times, and output path, as durable records. Execution SHALL be process-local; on server restart, Jobs that were `running` SHALL be marked `interrupted`.

#### Scenario: Restart marks interrupted
- **WHEN** the server restarts while two bash Jobs are running
- **THEN** both are recorded `interrupted` and listed as such in `/tasks`

### Requirement: Completion notices
(P1) When a Job started by a Session ends, the system SHALL admit a notice into that Session with `delivery: queue` containing the Job ID, status, exit code, duration and the last 20 lines of output, unless the Job was started with `notify: false`.

#### Scenario: Tests finish while idle
- **WHEN** a background `cargo test` Job exits 0 while the Session is idle
- **THEN** a notice is admitted and the agent starts a Turn to process it

### Requirement: Reading job output
(P1) The model SHALL read Job output with the `read` tool on the Job's output file path, using `offset`/`limit` paging; output files SHALL be capped at `background.max_output_mb` (default 64) with oldest bytes dropped and a truncation marker.

#### Scenario: Tail a log
- **WHEN** the model reads the last 200 lines of a running Job's log file
- **THEN** it receives the current content without stopping the Job

### Requirement: Monitor tool
(P1) The system SHALL provide a `monitor` tool that starts a background source (a command's stdout/stderr, a file tail, or a `ws://`/`wss://` WebSocket) and feeds matching lines into the Session as queued messages. It SHALL accept `filter` (regex), `max_lines_per_minute` (default 30, excess summarized as `N lines suppressed`), `until` (regex that ends the monitor when matched) and `timeout_seconds` (default 3600).

#### Scenario: Watch for errors
- **WHEN** the model monitors `tail -F app.log` with filter `ERROR|panic`
- **THEN** only matching lines are delivered, at most 30 per minute

#### Scenario: Stop on condition
- **WHEN** a monitor has `until: "Deployment complete"`
- **THEN** the monitor ends when that line appears and a final notice is delivered

### Requirement: Tasks view
(P1) `/tasks` (aliases `/ps`) SHALL list the Session's Jobs, background subagents and workflow runs with kind, status, elapsed time and last output line, and SHALL let the user open output, attach (PTY), or stop each entry. `/stop` SHALL stop all running Jobs of the Session after confirmation.

#### Scenario: Stop one job
- **WHEN** the user selects a running Job in `/tasks` and chooses Stop
- **THEN** the Job is cancelled and listed as `cancelled`

### Requirement: Stop kills process trees
(P1) Stopping a Job SHALL send SIGTERM to its whole process group (Windows: terminate the Job Object), wait up to 5 seconds, then SIGKILL remaining processes, and record `cancelled`. Model access SHALL be through a `task_stop` tool taking the Job ID.

#### Scenario: Child processes cleaned up
- **WHEN** a stopped Job had spawned grandchild processes
- **THEN** all of them are terminated

### Requirement: Concurrency limits
(P1) A Session SHALL run at most `background.max_jobs` (default 20) concurrent Jobs (excluding subagents and workflow agents, which have their own caps); further starts SHALL fail with `Background job limit reached (20)`.

#### Scenario: Limit hit
- **WHEN** a Session with 20 running Jobs starts another
- **THEN** the start fails with the limit message

### Requirement: Jobs end with their session
(P1) Deleting a Session SHALL cancel its Jobs. Interrupting a Session's Drain SHALL NOT cancel background Jobs. Jobs SHALL keep running when all clients disconnect.

#### Scenario: Interrupt keeps background jobs
- **WHEN** the user presses Esc during a Turn while a dev server Job runs
- **THEN** the Turn stops and the dev server keeps running

### Requirement: PTY sessions
(P1) The system SHALL provide Location-scoped PTYs for interactive programs, running the requested command or the preferred shell with `TERM=xterm-256color` and `CYBER_TERMINAL=1`, retaining up to 2 MiB of output per PTY, and letting clients attach with an output cursor (omitted replays the buffer, `-1` tails) over a WebSocket with a single-use 60-second ticket.

#### Scenario: Reattach replays output
- **WHEN** a client reconnects to a PTY with no cursor
- **THEN** it receives the retained buffer, then live output

### Requirement: Model interaction with PTYs
(P1) The model SHALL be able to start a PTY Job (`pty_start`), send input (`pty_write`, including control keys such as `ctrl-c`) and read the screen or recent output (`pty_read`), subject to `bash` permission on the command, so it can drive REPLs and interactive installers.

#### Scenario: Drive a REPL
- **WHEN** the model starts `python3` in a PTY and writes `print(2+2)\n`
- **THEN** `pty_read` returns output containing `4`

### Requirement: Notify tool
(P1) The system SHALL provide a `notify` tool that sends a desktop notification and, when remote-control is connected, a push notification to the user's Devices, with `title` (max 80 chars) and `body` (max 300 chars). Notifications SHALL be rate-limited to 10 per Session per hour, and `notify` SHALL be denied in `plan` Mode only when configured.

#### Scenario: Long task done
- **WHEN** the model calls `notify` after a 40-minute migration finishes
- **THEN** the user gets a desktop notification and a phone push when a Device is paired

### Requirement: Send file to user
(P1) The system SHALL provide a `send_file` tool that delivers files from the Location (max 25 MB each, max 10 per call) with an optional caption to the user's connected clients and Devices, recording a message part with the file reference.

#### Scenario: Deliver a report
- **WHEN** the model calls `send_file` with `report.pdf`
- **THEN** the file is shown as a downloadable attachment in the TUI and on paired Devices

### Requirement: Background job events
(P1) The system SHALL publish `job.started.1`, `job.output.1` (live-only, not durable), `job.ended.1` and `job.cancelled.1` on the event stream, and hooks SHALL be able to subscribe to `JobEnded`.

#### Scenario: Hook on job end
- **WHEN** a background build Job ends with a non-zero exit code
- **THEN** a `JobEnded` hook receives the Job JSON with `exit_code`

### Requirement: Background commands from the user
(P1) In the TUI, prefixing a shell-mode command with `&` (for example `& npm run dev`) SHALL start it as a background Job of the Session without involving the model, and Ctrl+B during a running foreground command SHALL move it to the background.

#### Scenario: Move to background
- **WHEN** the user presses Ctrl+B while a foreground `bash` tool call runs
- **THEN** the command becomes a background Job and the Turn continues with the Job ID as the result
