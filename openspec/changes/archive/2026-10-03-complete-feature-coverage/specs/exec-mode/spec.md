## ADDED Requirements

### Requirement: Streaming input and partial output
(P1) `--input-format stream-json` SHALL read JSON lines from stdin for the life of the process: `{ "type": "user", "text" | "parts", "delivery"? }` admits a prompt, `{ "type": "interrupt" }` interrupts the Drain, `{ "type": "permission_reply", "request_id", "reply", "message"? }` and `{ "type": "question_reply", "request_id", "answers" }` answer pending requests (which are emitted as `permission_request` and `question` events instead of being auto-denied), and `{ "type": "end" }` or EOF ends the run after the current Drain goes idle. Each prompt SHALL produce its own `result` event and the process SHALL exit only on `end`/EOF. `--include-partial-messages` SHALL add `text_delta` and `reasoning_delta` events; `--include-hook-events` SHALL add `hook_event` lines (hook id, event, outcome, decision); `--output-last-message <file>` SHALL write the final assistant text of the last Turn to that file. All added types SHALL be declared in the `init` event's `schema_version` 2.

#### Scenario: SDK-style multi-turn driver
- **WHEN** a program starts `cyber exec --input-format stream-json --format stream-json` and writes two `user` lines followed by `end`
- **THEN** stdout carries two `result` events in order and the process exits 0 after the second Drain goes idle

#### Scenario: Permission answered over stdin
- **WHEN** the model needs approval for `bash` and the driver writes a `permission_reply` with `reply: "once"`
- **THEN** the command runs and no `permission_denied` event is emitted
