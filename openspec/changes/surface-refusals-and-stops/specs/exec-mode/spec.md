## ADDED Requirements

### Requirement: Refused and truncated answers in exec
(P1) When a run ends on its own and its last finished step stopped with `content_filter`, the result's `stop_reason` SHALL be `refusal`; with `length`, `max_tokens`. Other runs keep `end_turn` or `error`, and a stop reason set by a limit, timeout or interrupt takes precedence. In text mode, unless `--quiet`, exec SHALL print one `warning:` line on stderr for each step that stopped this way. The exit code is unchanged.

#### Scenario: Refusal in exec
- **WHEN** `cyber exec --format json` runs a prompt the model refuses with `content_filter`
- **THEN** `result` holds the refusal text, `stop_reason` is `refusal` and the exit code is 0
