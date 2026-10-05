## MODIFIED Requirements

### Requirement: Permission modes
(P0) The system SHALL support Modes `default` and `plan` in P0, adding `accept-edits`, `auto`, `dont-ask` and `bypass` in P1, selectable per Session (`--mode`, `mode` config, agent `permission_mode`, `/mode <name>`, `POST /api/v1/sessions/:id/mode`). The TUI SHALL cycle Modes with Shift+Tab through `default → accept-edits → plan → auto → default` on P1 builds (`default → plan → default` on P0 builds); `bypass` and `dont-ask` SHALL be reachable only through `/mode`, flags or config. A Mode change SHALL apply at the next Turn and publish `session.mode.switched.1`, as specified by session-runtime. The UI SHALL show a requested change as pending until effective; interrupt SHALL be offered when immediate cancellation is needed.

#### Scenario: Cycling modes
- **WHEN** the user presses Shift+Tab twice from `default` on a P1 build
- **THEN** the Session mode becomes `plan`

### Requirement: auto mode classifier
(P1) In `auto`, every request that would be `ask` SHALL be reviewed by a classifier using `model_roles.evaluator`, else `small_model`. The classifier SHALL receive the tool call, the last 20 messages, the Location and the user's stated boundaries. It SHALL return `allow` or `block` with a reason. The system SHALL block irreversible or out-of-scope actions (force pushes, deploys, deletes outside the Location, credential access, data exfiltration to non-allowlisted hosts). After 3 consecutive blocks in one Drain, or when the classifier is unavailable, the system SHALL fall back to `ask`, or to `deny` when no user is attached. Each decision SHALL be recorded as `permission.auto_decided.1` with the reason.

#### Scenario: Force push blocked
- **WHEN** an `auto` Session runs `git push --force origin main`
- **THEN** the classifier blocks it and the model receives `Blocked by auto mode: <reason>`

#### Scenario: Fallback after repeated blocks
- **WHEN** the classifier blocks 3 calls in a row
- **THEN** the next ask is shown to the user as a normal prompt
