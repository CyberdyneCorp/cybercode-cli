## MODIFIED Requirements

### Requirement: Background bash
(P1) The `bash` tool SHALL accept `background: true` (the parameter defined in `builtin-tools`), start the command as a Job (`job_` ID) and return immediately with the Job ID and the path of its output file under `<data>/jobs/<job_id>.log`. The Job SHALL keep running after the Turn ends, under the same sandbox and permissions as foreground commands.

#### Scenario: Start a dev server
- **WHEN** the model runs `npm run dev` with `background: true`
- **THEN** the tool returns at once with the Job ID and log path, and the conversation continues

### Requirement: Monitor tool
(P1) The system SHALL provide a `monitor` tool with input `{ command? | url? | path?, filter?, max_lines_per_minute? (default 30), until?, timeout_seconds? (default 3600) }`, exactly one of `command` (stdout and stderr of a process), `url` (a `ws://` or `wss://` WebSocket) or `path` (a file tail). Lines matching `filter` (regex, default all) SHALL be admitted into the Session as prompts with `delivery: queue` and `source: monitor`, so they never interrupt a running Turn. Excess lines beyond `max_lines_per_minute` SHALL be summarized as `N lines suppressed`. The monitor SHALL end when `until` (regex) matches, on `timeout_seconds`, or when stopped, delivering a final notice. It SHALL return the `job_` ID. This requirement is the only definition of the tool's contract.

#### Scenario: Watch for errors
- **WHEN** the model monitors `tail -F app.log` with filter `ERROR|panic`
- **THEN** only matching lines are delivered, at most 30 per minute

#### Scenario: Stop on condition
- **WHEN** a monitor has `until: "Deployment complete"`
- **THEN** the monitor ends when that line appears and a final notice is delivered
