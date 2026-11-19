## MODIFIED Requirements

### Requirement: plan mode
(P0) In `plan`, the system SHALL allow only tools annotated `read_only`, plus writing the plan file `<location>/.cyber/plans/<session-slug>.md`, and SHALL deny every other mutating action with `Plan mode is read-only. Present the plan with plan_exit.` The system SHALL provide the tools `plan_enter {}` (switch the Session to `plan` at the next Turn) and `plan_exit { summary }` (present the plan file for approval). `plan_exit` SHALL route a question to the user with the choices `Approve and build (default)`, `Approve with accept-edits`, `Approve with auto` (only when `auto` is available) and `Request changes`. An approval SHALL switch the Mode to the chosen one at the next Turn; `Request changes` SHALL return the user's feedback as the tool result and keep `plan`. When the planner model is the Session's active model (`provider-catalog` → Planner and executor models), an approval's tool result SHALL also carry the approved plan text, so the executor model receives the exact plan it is to carry out. In non-interactive Sessions both tools SHALL be denied. Planning is a Mode, not an agent: there is no built-in `plan` agent.

#### Scenario: Write blocked in plan mode
- **WHEN** a `plan` Session calls `edit` on `src/lib.rs`
- **THEN** the call is denied with `Plan mode is read-only. Present the plan with plan_exit.`

#### Scenario: Plan approved with accept-edits
- **WHEN** the model calls `plan_exit` and the user chooses `Approve with accept-edits`
- **THEN** the Session's Mode becomes `accept-edits` at the next Turn and `session.mode.switched.1` is published

#### Scenario: Changes requested
- **WHEN** the user chooses `Request changes` and types `split the migration into two steps`
- **THEN** the tool returns that feedback to the model and the Session stays in `plan`

#### Scenario: Approved plan handed to the executor
- **WHEN** the planner is active and the user approves a plan whose file says `1. Add the migration`
- **THEN** the `plan_exit` result contains `Approved plan` followed by `1. Add the migration`, and the next Turn runs on the executor with that result in its history
