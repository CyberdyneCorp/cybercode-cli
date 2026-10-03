## MODIFIED Requirements

### Requirement: Decision schema
(P1) A hook decision SHALL be a JSON object with optional fields: `decision` (`allow`, `deny`, `ask`), `reason`, `updated_input` (PreToolUse only, replaces tool input after re-validation against the tool schema), `additional_context` (text admitted as a system message at the next Safe Boundary; for `PreCompact` it is appended to the summary instructions instead), `continue` (false stops the Drain after the current Turn), `stop_reason`, and `suppress_output` (hide the hook's output from the transcript). Fields not valid for the event SHALL be ignored with a debug log.

#### Scenario: Input rewritten
- **WHEN** a `PreToolUse` hook returns `{"updated_input": {"command": "npm test -- --ci"}}` for a bash call
- **THEN** the bash tool runs `npm test -- --ci` and the transcript shows the rewrite

#### Scenario: Invalid rewritten input
- **WHEN** `updated_input` fails the tool's input schema
- **THEN** the call is denied with `hook produced invalid tool input`
