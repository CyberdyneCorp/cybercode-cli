## ADDED Requirements

### Requirement: Tool recovery contract
(P0) Each tool SHALL declare `retry_safety: read_only|idempotent|reconcile|never` (default `never`). Before dispatch the runtime SHALL persist the call ID, input digest, permission decision, and attempt state. Idempotent external APIs SHALL receive a stable operation key across retries. Dispatched calls with no durable result SHALL enter `outcome_unknown`; read-only calls may be retried, idempotent calls only with a documented provider guarantee, and other calls only after read-only reconciliation establishes the outcome or the user explicitly authorizes another attempt. A read-only reconciliation handler MAY report `succeeded`, `not_applied` or `unknown` with evidence. Shell commands SHALL default to `never`; a process exit or termination alone SHALL NOT prove absence of effects. Permission checks SHALL be repeated for a retry. The model SHALL NOT evade a recovery hold by issuing an equivalent mutation under a new call ID; when equivalence cannot be safely established, the Session SHALL pause mutating execution pending reconciliation.

#### Scenario: PR created before crash
- **WHEN** a create-PR call succeeds remotely and the server crashes before settlement
- **THEN** resume records an unknown outcome, queries for the PR using the operation evidence, and does not blindly create a second PR

## MODIFIED Requirements

### Requirement: Failure versus defect semantics
(P0) The system SHALL turn an expected tool failure (`ToolFailure { message }`) into a model-visible error result carrying only its message. Panics, defects and interruptions SHALL NOT become tool results; they SHALL propagate to the session runtime, which records `outcome_unknown` for dispatched mutations without a known outcome, or an interrupted/crashed failure for calls known not to have mutated state, and logs defects with a reference ID.

#### Scenario: Defect not leaked to model
- **WHEN** a plugin tool throws an unexpected exception containing a stack trace
- **THEN** the model sees `Tool crashed: err_<id>` and the stack trace is only in the log

### Requirement: Invocation context
(P0) The system SHALL invoke every tool with a context `{ session_id, agent, message_id, call_id, location, mode, abort_signal, ask(permission_request), metadata(update) }`. The `abort_signal` SHALL fire when the Session is interrupted or the Turn is cancelled, and tools SHALL stop work within 2 seconds of the signal.

#### Scenario: Interrupt aborts a running tool
- **WHEN** the user interrupts a Session while `bash` is running
- **THEN** the abort signal fires, the process tree is terminated, and the call settles with its known outcome or `outcome_unknown` if dispatched side effects cannot be established

### Requirement: Managed tool output files
(P0) The system SHALL write the complete text of every truncated output to an exclusively created file `<data>/tool-output/tool_<ulid>` and record the path in the call's `output_paths` metadata. It SHALL delete unpinned files older than 7 days in an hourly cleanup, preserving references required by active Sessions, workflows, recovery records and backups per storage-events. Failure to write the overflow file SHALL fail the call operationally instead of returning lossy output.

#### Scenario: Overflow file readable later
- **WHEN** a truncated call recorded `<data>/tool-output/tool_01J...`
- **THEN** the model can `read` that path without an `external_directory` prompt
