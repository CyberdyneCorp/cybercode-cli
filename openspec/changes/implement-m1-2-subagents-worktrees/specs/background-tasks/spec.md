## MODIFIED Requirements

### Requirement: Job registry
(P1) The system SHALL record each Job with `id`, `session_id`, `kind` (`bash`, `monitor`, `pty`, `subagent`, `workflow`), `command` or description, `status` (`running`, `completed`, `error`, `cancelled`, `interrupted`), `exit_code`, start and end times, and output path, as durable records. Execution SHALL be process-local; on server restart, Jobs that were `running` SHALL be marked `interrupted`.

#### Scenario: Restart marks interrupted
- **WHEN** the server restarts while two bash Jobs are running
- **THEN** both are recorded `interrupted` and listed as such in `/tasks`

### Requirement: Tasks view
(P1) `/tasks` (aliases `/ps`) SHALL list the Session's Jobs, background subagents and workflow runs with kind, status, elapsed time and last output line, and SHALL let the user open output, attach (PTY), or stop each entry. `/stop` SHALL stop all running Jobs of the Session after confirmation.

#### Scenario: Stop one job
- **WHEN** the user selects a running Job in `/tasks` and chooses Stop
- **THEN** the Job is cancelled and listed as `cancelled`

#### Scenario: Open a command's output
- **WHEN** the user opens a background command or monitor in `/tasks`
- **THEN** the end of its log is shown, read through `GET /api/v1/jobs/{id}/output`
- **AND** each command or monitor row shows the last line of its log

### Requirement: Jobs end with their session
(P1) Deleting a Session SHALL cancel its Jobs. Interrupting a Session's Drain SHALL NOT cancel background Jobs. Jobs SHALL keep running when all clients disconnect.

#### Scenario: Interrupt keeps background jobs
- **WHEN** the user presses Esc during a Turn while a dev server Job runs
- **THEN** the Turn stops and the dev server keeps running

### Requirement: Background job events
(P1) The system SHALL publish `job.started.1`, `job.output.1` (live-only, not durable), `job.ended.1`, `job.cancelled.1` and the durable handback acknowledgement `job.notified.1` on the event stream, and hooks SHALL be able to subscribe to `JobEnded`.

#### Scenario: Hook on job end
- **WHEN** a background build Job ends with a non-zero exit code
- **THEN** a `JobEnded` hook receives the Job JSON with `exit_code`

#### Scenario: Interrupted subagent tasks produce one recoverable notice
- **WHEN** the application restarts with an unfinished process-local subagent Job
- **THEN** it SHALL mark the Job interrupted without redispatch and admit one queue handback
- **AND** repeating recovery after admission but before notification acknowledgement SHALL NOT duplicate the inbox row

#### Scenario: Stop releases a child permission request
- **WHEN** a background child is waiting for permission and its Job is stopped
- **THEN** the child SHALL settle and its owned permission/question requests SHALL be closed on the child and routed parent streams

### Requirement: Monitor tool
(P1) The system SHALL provide a `monitor` tool with input `{ command? | url? | path?, filter?, max_lines_per_minute? (default 30), until?, timeout_seconds? (default 3600) }`, exactly one of `command` (stdout and stderr of a process), `url` (a `ws://` or `wss://` WebSocket) or `path` (a file tail). Lines matching `filter` (regex, default all) SHALL be admitted into the Session as prompts with `delivery: queue` and `source: monitor`, so they never interrupt a running Turn. Excess lines beyond `max_lines_per_minute` SHALL be summarized as `N lines suppressed`. The monitor SHALL end when `until` (regex) matches, on `timeout_seconds`, or when stopped, delivering a final notice. It SHALL return the `job_` ID. This requirement is the only definition of the tool's contract.

#### Scenario: Watch for errors
- **WHEN** the model monitors `tail -F app.log` with filter `ERROR|panic`
- **THEN** only matching lines are delivered, at most 30 per minute

#### Scenario: Stop on condition
- **WHEN** a monitor has `until: "Deployment complete"`
- **THEN** the monitor ends when that line appears and a final notice is delivered

#### Scenario: Monitor record and rate window
- **WHEN** a monitor starts
- **THEN** it SHALL be a Job of kind `monitor` whose `command` is the command, the URL or `tail -F <path>`, counted against `background.max_jobs`
- **AND** every line SHALL be written to its log, while only matching lines are queued as `Monitor <job_id>: <line>`
- **AND** the lines suppressed in a 60-second window SHALL be summarized once when the window ends or the monitor ends, before the final notice
- **AND** a file source SHALL deliver only lines appended after the monitor started, reading a truncated file again from its start

### Requirement: Background commands from the user
(P1) In the TUI, prefixing a shell-mode command with `&` (for example `& npm run dev`) SHALL start it as a background Job of the Session without involving the model, and Ctrl+B during a running foreground command SHALL move it to the background.

#### Scenario: Move to background
- **WHEN** the user presses Ctrl+B while a foreground `bash` tool call runs
- **THEN** the command becomes a background Job and the Turn continues with the Job ID as the result

#### Scenario: Background command and handoff through the API
- **WHEN** a client posts `POST /api/v1/sessions/{id}/shell` with `background: true`
- **THEN** the command SHALL start as a `bash` Job under the sandbox without a permission prompt, a message or a Turn, and the response SHALL carry the Job
- **WHEN** a client posts `POST /api/v1/sessions/{id}/background`
- **THEN** every running foreground `bash` call of the Session SHALL become a background Job whose log starts with the output captured so far, and the call SHALL settle with the Job's ID
- **AND** with no foreground `bash` call running the request SHALL fail with 409 and nothing changes
- **AND** when the Job cannot be admitted (for example at `background.max_jobs`) the command SHALL keep running in the foreground

### Requirement: PTY sessions
(P1) The system SHALL provide Location-scoped PTYs for interactive programs, running the requested command or the preferred shell with `TERM=xterm-256color` and `CYBER_TERMINAL=1`, retaining up to 2 MiB of output per PTY, and letting clients attach with an output cursor (omitted replays the buffer, `-1` tails) over a WebSocket with a single-use 60-second ticket.

#### Scenario: Reattach replays output
- **WHEN** a client reconnects to a PTY with no cursor
- **THEN** it receives the retained buffer, then live output

#### Scenario: Ticketed attach
- **WHEN** a client posts `POST /api/v1/jobs/{id}/pty/ticket` with the server's credentials for a running PTY Job
- **THEN** it SHALL receive a ticket bound to that PTY that expires after 60 seconds
- **AND** `GET /api/v1/jobs/{id}/pty/connect?ticket=<ticket>&cursor=<n>` SHALL accept the ticket once as the socket's only credential; a reused, expired, forged or other-PTY ticket SHALL be refused with 401, and any attempt SHALL consume the ticket
- **AND** the socket SHALL first send `{"type":"attached","cursor":<n>}`, then the output from that cursor as binary frames; client frames SHALL be input, and `{"type":"exit","cursor":<n>}` SHALL follow the program's exit
- **AND** a PTY whose program exited SHALL no longer issue tickets (404 `PtyNotFoundError`)

#### Scenario: Windows
- **WHEN** the server runs on Windows
- **THEN** the PTY tools SHALL be absent until ConPTY is wired to the sandboxed launch

### Requirement: Model interaction with PTYs
(P1) The model SHALL be able to start a PTY Job (`pty_start`), send input (`pty_write`, including control keys such as `ctrl-c`) and read the screen or recent output (`pty_read`), subject to `bash` permission on the command, so it can drive REPLs and interactive installers.

#### Scenario: Drive a REPL
- **WHEN** the model starts `python3` in a PTY and writes `print(2+2)\n`
- **THEN** `pty_read` returns output containing `4`

#### Scenario: Tool contracts
- **WHEN** the model calls `pty_start({ command?, cols?, rows?, description? })`
- **THEN** the command SHALL be authorized and sandboxed exactly as a `bash` command (the removal guard included), and SHALL run as a Job of kind `pty` counted against `background.max_jobs`, logged to its output path and stopped by `task_stop`
- **AND** `pty_write({ job_id, input?, keys? })` SHALL send `input` as typed followed by named keys (`enter`, `tab`, `esc`, `backspace`, arrows, `home`, `end`, `delete`, `ctrl-a`…`ctrl-z`), checked against the `bash` rules for the PTY's command
- **AND** `pty_read({ job_id, mode?, wait_ms?, cursor?, max_bytes? })` SHALL return, in `output` mode, the output since the previous read as plain text with escape sequences removed, waiting up to `wait_ms` (default 1000) for the program to print and go quiet, and in `screen` mode the terminal's current screen
- **AND** a Session SHALL NOT read or write another Session's PTY

### Requirement: Notify tool
(P1) The system SHALL provide a `notify` tool that sends a desktop notification and, when remote-control is connected, a push notification to the user's Devices, with `title` (max 80 chars) and `body` (max 300 chars). Notifications SHALL be rate-limited to 10 per Session per hour, and `notify` SHALL be denied in `plan` Mode only when configured.

#### Scenario: Long task done
- **WHEN** the model calls `notify` after a 40-minute migration finishes
- **THEN** the user gets a desktop notification and a phone push when a Device is paired

#### Scenario: Limits and record
- **WHEN** the model calls `notify`
- **THEN** control characters SHALL be removed and longer fields cut with an ellipsis to 80 and 300 characters, the durable event `session.notification.sent.1` (`call_id`, `title`, `body`) SHALL be recorded, and clients SHALL show it as a desktop notification
- **AND** the eleventh call within an hour SHALL fail with `Notification limit reached (10 per hour)`
- **AND** the permission action SHALL be `notify`, allowed by default; in `plan` Mode the tool SHALL stay offered unless `background.deny_notify_in_plan` is true

### Requirement: Send file to user
(P1) The system SHALL provide a `send_file` tool that delivers files from the Location (max 25 MB each, max 10 per call) with an optional caption to the user's connected clients and Devices, recording a message part with the file reference.

#### Scenario: Deliver a report
- **WHEN** the model calls `send_file` with `report.pdf`
- **THEN** the file is shown as a downloadable attachment in the TUI and on paired Devices

#### Scenario: Checks, copy and download
- **WHEN** the model calls `send_file({ paths, caption? })`
- **THEN** each path SHALL pass the `read` permission, the external-directory check and `deny_read`, and a call with more than 10 paths or a file over 25 MB SHALL fail before anything is sent
- **AND** each file SHALL be copied to managed storage, and `session.files.sent.1` SHALL record the `call_id`, `message_id`, caption and each file's ID, name, size, media type and SHA-256
- **AND** `GET /api/v1/sessions/{id}/files/{file_id}` SHALL return the copy as an attachment, unchanged by later edits of the source
