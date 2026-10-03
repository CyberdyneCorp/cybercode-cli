## MODIFIED Requirements

### Requirement: Supported events
(P1) The system SHALL emit hook events `Setup` (first Session in a Location after install or `cyber --init`), `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PostToolBatch` (after all calls of one Turn settle), `PermissionRequest`, `PermissionDenied` (MAY return `{ "decision": "retry", "updated_input" }` to re-issue the call once), `Stop`, `StopFailure`, `Interrupt`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `InstructionsLoaded` (an instruction or rule file entered the context), `PreModelSwitch`, `PostModelSwitch`, `Elicitation`, `ElicitationResult`, `DirectoryAdded`, `Notification`, `FileChanged`, `CwdChanged`, `ConfigChange`, `TaskCreated`, `TaskCompleted`, `WorktreeCreate`, `WorktreeRemove` and `JobEnded`, and (P2) `GoalEvaluated`, `GoalCompleted`, `WorkflowRunStart`, `WorkflowRunEnd`, `LoopIteration`, `ScheduleRun`, `TeammateIdle` and `MessageReceived`. Each event SHALL carry a common envelope `{ event, session_id, location: { directory, workspace? }, project_id, agent, mode, timestamp }` plus event-specific fields.

#### Scenario: PostToolUse payload
- **WHEN** an `edit` tool call completes successfully
- **THEN** each matching `PostToolUse` hook receives the envelope plus `tool_name`, `tool_input`, `tool_output`, `call_id` and `duration_ms`

#### Scenario: MessageReceived for cross-session messages
- **WHEN** another session's message is delivered into session `ses_1`
- **THEN** `MessageReceived` hooks run with `from_session`, `from_machine` and `text` before the message is shown to the model

#### Scenario: Denied call retried by a hook
- **WHEN** a `PermissionDenied` hook returns `{ "decision": "retry", "updated_input": { "command": "npm test -- --ci" } }`
- **THEN** the rewritten call is evaluated once more and runs if the rules allow it

## ADDED Requirements

### Requirement: Conditional, one-shot and annotated handlers
(P1) A handler MAY set `if` (`{ field, matches }`: a dotted payload field and a regex), `once: true` (run at most once per Session), `status_message` (shown in the client while the handler runs) and `system_message` (shown to the user, not the model, when the handler completes). A handler whose `if` does not match SHALL be skipped without logging an execution event.

#### Scenario: Run only for one branch
- **WHEN** a `Stop` hook has `if: { field: "git.branch", matches: "^release/" }` and the Session is on `main`
- **THEN** the handler is skipped

### Requirement: Agent handlers
(P2) A handler of `type: "agent"` SHALL run a read-only subagent (`explore` by default, or `agent`) with the handler's `prompt`, the event JSON and a Budget of `max_turns` (default 5), and SHALL parse its structured result `{ decision, reason }` as the decision. Agent handlers SHALL count toward session cost and SHALL time out like other handlers.

#### Scenario: Agent judges a diff
- **WHEN** a `PostToolUse` agent handler is asked to check whether an edit touched generated files
- **THEN** the subagent reads the diff with read-only tools and returns `{ "decision": "deny", "reason": "edited a generated file" }`, which blocks nothing but is shown to the user
