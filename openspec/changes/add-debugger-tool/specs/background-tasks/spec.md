## MODIFIED Requirements

### Requirement: Job registry
(P1) The system SHALL record each Job with `id`, `session_id`, `kind` (`bash`, `monitor`, `pty`, `subagent`, `workflow`, `debug` (a debug session owned by the debugging capability)), `command` or description, `status` (`running`, `completed`, `error`, `cancelled`, `interrupted`), `exit_code`, start and end times, and output path, as durable records. Execution SHALL be process-local; on server restart, Jobs that were `running` SHALL be marked `interrupted`.

#### Scenario: Restart marks interrupted
- **WHEN** the server restarts while two bash Jobs are running
- **THEN** both are recorded `interrupted` and listed as such in `/tasks`


#### Scenario: Debug session Job
- **WHEN** the model launches a program with the `debug` tool
- **THEN** a Job with kind `debug` is recorded and listed in `/tasks`, and a server restart marks it `interrupted`
