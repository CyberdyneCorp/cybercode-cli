## ADDED Requirements

### Requirement: Usage limit waits in exec
(P1) `cyber exec` SHALL create its Sessions with `unattended: true`, so a usage limit with a known reset waits as `session-runtime` → Usage limit wait defines. In `stream-json` mode each wait change SHALL be one line of type `usage_limit` with `status` (`waiting`, `resumed` or `abandoned`), `message_id`, and for `waiting` `resets_at_ms` and `resume_at_ms`, for `abandoned` `reason`. This type is an additive change within `schema_version` 1. In text mode, unless `--quiet`, exec SHALL print one stderr line when a wait starts, naming the local reset and resume times, and one when it resumes. When `--timeout` would expire before the planned resume time, exec SHALL stop the run at once with `stop_reason` `timeout` and exit code 4.

#### Scenario: Stream of a waited run
- **WHEN** a stream-json run hits a usage limit that resets in 2 minutes and then succeeds
- **THEN** stdout has a `usage_limit` line with `status: "waiting"`, then one with `status: "resumed"`, then `assistant_text` and a `result` with exit code 0

#### Scenario: Timeout before the reset
- **WHEN** `cyber exec --timeout 10m` hits a usage limit that resets in 40 minutes
- **THEN** exec stops the run right after the `waiting` event with `stop_reason: "timeout"` and exit code 4
