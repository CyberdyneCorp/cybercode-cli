## ADDED Requirements

### Requirement: Durable local schedule occurrences
(P2) Local cron and persistent schedules SHALL pin an IANA timezone at creation, defaulting to the resolved host timezone, and accept an explicit timezone override. Cron SHALL use five fields with one-minute resolution. A daylight-saving gap SHALL skip the nonexistent local occurrence; a repeated local minute SHALL fire only at its first occurrence. Durable schedule revision and occurrence identities SHALL fence admission so repeated ticks or restarts cannot enqueue the same occurrence twice. Pause, removal and revision SHALL invalidate stale dispatch claims. This admission guarantee SHALL NOT be described as exactly-once external execution.

#### Scenario: Restart after admission
- **WHEN** the service restarts after persisting an occurrence's admitted prompt but before advancing its next-due cursor
- **THEN** recovery recognizes the admitted occurrence and does not enqueue it again

#### Scenario: Host timezone changes
- **WHEN** the host changes timezone after a schedule was created
- **THEN** that schedule continues to use its stored timezone and displays it to the owner

#### Scenario: Repeated or nonexistent clock minute
- **WHEN** a cron-selected local minute repeats or does not exist during a daylight-saving transition
- **THEN** it runs once at the first occurrence or is skipped respectively

### Requirement: Bounded schedule recovery and unattended authority
(P2) Persistent local schedules SHALL preserve the existing skip/run-once missed-run and skip/coalesce overlap policies with a bounded backlog, default to dont-ask, and obey workspace trust, user/managed bypass policy and applicable budgets. They SHALL NOT create interactive presence or automatically retry an uncertain external execution. CLI/API/SDK details SHALL expose timezone, next/last occurrences and skipped, denied, unknown and budget-stopped outcomes. They SHALL state that execution requires the local server/service to be running.

#### Scenario: Several missed runs
- **WHEN** the service recovers after five missed occurrences with run-once policy
- **THEN** at most one catch-up occurrence is admitted before the regular schedule resumes

#### Scenario: Pause races with a due tick
- **WHEN** a schedule is paused after a tick reads it but before admission commits
- **THEN** its revision fence prevents that stale tick from admitting a prompt

#### Scenario: No approver attached
- **WHEN** a persistent schedule reaches a tool request that requires approval in its default dont-ask Mode
- **THEN** the request is denied under unattended policy rather than waiting on a fabricated browser attachment or approving itself
