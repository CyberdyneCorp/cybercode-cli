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
