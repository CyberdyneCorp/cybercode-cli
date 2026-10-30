## ADDED Requirements

### Requirement: Usage limit wait
(P1) When a Turn fails with `QuotaExceeded` before any assistant output, after the retry policy and the Model fallback chain (`provider-catalog`) have not produced a stream, the system SHALL decide whether to wait for the usage limit to reset instead of failing the Turn. It SHALL wait only when all of these hold:
- `usage_limit.mode` (`configuration` → Usage limit settings) is `always`, or it is `unattended` and the Session is unattended;
- the error carries the time the limit resets (`resets_at`);
- the reset lies no more than `usage_limit.max_wait` after the start of the first wait of this Turn (now, when there was none);
- the planned resume time falls before every wall deadline that applies to the Session.

A Session SHALL be unattended when the server is not interactive, when it was created with `unattended: true`, when it is a Workflow agent, has an active Goal, has a loop iteration in flight, is a non-lead team member, or is a subagent whose parent is unattended.

The planned resume time SHALL be the reset time (or now, when the reset already passed) plus a 10 second margin plus a jitter of 0 to 30 seconds derived from the Session ID and the reset time. Waiting SHALL append, together with `session.step.failed.1` of the Turn (which carries `resets_at_ms`), a durable `session.usage_limit.waiting.1` with `message_id`, `resets_at_ms`, `resume_at_ms`, `started_ms` and `by` (`runtime`, or `user` for a scheduled resume). While it waits the Drain SHALL stay running, publish no `session.error`, not pause the active Goal and not run StopFailure hooks.

At the resume time the system SHALL append `session.usage_limit.resumed.1` and run the Turn again from the same durable history, as one new provider attempt with a fresh transient retry budget, after the budget checks of a new Turn. An interrupt during the wait SHALL append `session.usage_limit.abandoned.1` with reason `interrupted` and end the Drain as an interrupted Turn would. A shutdown SHALL leave the wait pending. The wait SHALL hold no budget reservation.

When the system does not wait, the Turn SHALL fail as before and, except for an interactive Session under `unattended`, the failure message SHALL say why it did not wait. When the planned resume falls past a wall deadline, the run SHALL end with that deadline's outcome: a Session or Workflow Run budget fails the Drain with `BudgetExceeded` naming `max_wall_seconds`, and a Goal's or loop's budget ends it as `budget_exceeded`.

`POST /api/v1/sessions/{id}/usage-limit/resume` SHALL schedule a wait for an idle Session whose last finished step failed on a usage limit with a known reset, applying `max_wait` and the deadlines but not the mode, and start a Drain that waits and then runs the Turn again. It SHALL fail with `SessionBusyError` for a running Session and with `InvalidRequestError` when there is nothing to resume or the wait is not allowed. The Session view SHALL carry the pending wait as `usage_limit`, else null.

#### Scenario: Unattended run waits and continues
- **WHEN** a `cyber exec` Session's Turn fails before output with `usage_limit_reached` and `resets_at` 40 minutes ahead, and `usage_limit` is unset
- **THEN** `session.usage_limit.waiting.1` records a `resume_at_ms` between 40 m 10 s and 40 m 40 s ahead, no `session.error` is published, and after that time `session.usage_limit.resumed.1` is appended and the same request is sent again

#### Scenario: Reset beyond max_wait
- **WHEN** an unattended Turn hits a usage limit that resets in 7 hours with the default `max_wait` of 6 hours
- **THEN** the Turn fails and the error says `not waiting: the reset is more than usage_limit.max_wait (6h) away`

#### Scenario: Interactive Session does not wait
- **WHEN** a TUI Session with no Goal or loop hits a usage limit and `usage_limit.mode` is `unattended`
- **THEN** the Turn fails with `usage limit reached; resets at <time>` and no wait is recorded

#### Scenario: Unknown reset
- **WHEN** an unattended Turn fails with `QuotaExceeded` that carries no reset time
- **THEN** the Turn fails and the error says `not waiting: the provider did not say when the limit resets`

#### Scenario: Interrupt cancels the wait
- **WHEN** the user interrupts a Session that is waiting for a usage limit to reset
- **THEN** `session.usage_limit.abandoned.1` with reason `interrupted` is appended and the Session becomes idle without another provider request

#### Scenario: Wall deadline wins
- **WHEN** an unattended Session with a budget `max_wall_seconds` that ends in 10 minutes hits a usage limit that resets in 30 minutes
- **THEN** no wait is recorded and the Drain fails with `BudgetExceeded` naming `max_wall_seconds`

#### Scenario: Retry hits the limit again
- **WHEN** a waited Turn's retry fails with a new usage limit whose reset lies more than `max_wait` after the first wait started
- **THEN** the Turn fails with the `max_wait` note instead of waiting again

### Requirement: Usage limit wait recovery
(P1) At server start the system SHALL settle every pending usage limit wait. A top-level Session that is not a Workflow agent or a team member SHALL be resumed: its Drain SHALL wait for the time left (none when it passed), check the wall deadlines again, and then run the Turn again. A subagent, Workflow agent or team member SHALL have its wait abandoned with reason `restarted` and a `session.error`, so that its owner's own recovery decides what runs next. A wait whose resume now falls past a wall deadline SHALL be abandoned with reason `deadline` and end with that deadline's outcome. No Session SHALL be left with a pending wait and no Drain.

#### Scenario: Restart during a wait
- **WHEN** the server stops while an exec Session waits for a reset 20 minutes ahead and starts again 5 minutes later
- **THEN** the Session's Drain runs again, waits about 15 more minutes and then retries the Turn

#### Scenario: Restart abandons a child's wait
- **WHEN** the server restarts while a subagent Session waits for a usage limit reset
- **THEN** the child's wait is abandoned with reason `restarted` and the Session is idle
