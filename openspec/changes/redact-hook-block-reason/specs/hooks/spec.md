## ADDED Requirements

### Requirement: Hook output redaction
(P1) A hook's own text SHALL be redacted before anything shows or stores it: the `reason` and `stop_reason` of its decision (for a command handler that exits with code 2, its stderr) and the diagnostic of a failed handler. Redaction SHALL use the Location's tool output redactor (`tool-registry` → Tool output redaction), the same that applies to tool results, and SHALL apply to the tool result the model receives, live hook notices, merged decisions and the durable `hook.executed.1` receipt. After redaction each text SHALL be bounded to 4 KiB, cut on a character boundary and marked as truncated. Text without a secret SHALL stay as written. An `updated_input` SHALL be applied as the hook returned it.

#### Scenario: A blocking hook prints a token
- **WHEN** a `PreToolUse` command hook exits with code 2 and stderr `deploy refused for ghp_abcdefghijklmnop0123456789`
- **THEN** the call does not run, the model's tool error, the live notice and the hook receipt read `deploy refused for [REDACTED:github_token]`, and the token is in none of them or in the Session's events

#### Scenario: A long block reason
- **WHEN** a blocking hook writes 200 KB to stderr
- **THEN** the tool error and the receipt keep its first 4 KiB followed by a truncation marker

#### Scenario: A failing hook prints a token
- **WHEN** a `PostToolUse` command hook exits with code 1 and its stderr holds a token
- **THEN** the turn continues and the warning notice shows the stderr with the token replaced by `[REDACTED:github_token]`
