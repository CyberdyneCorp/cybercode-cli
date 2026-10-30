## ADDED Requirements

### Requirement: Resume at usage limit reset
(P1) When a Turn of the attached Session fails with `quota_exceeded` and the message names the reset time without a `not waiting` note, the error notice SHALL end with `/resume-at-reset to continue then`. `/resume-at-reset` SHALL call `POST /api/v1/sessions/{id}/usage-limit/resume` and show either the planned resume time or the refusal. While the attached Session has a pending usage limit wait, the footer SHALL show `⏸ usage limit · resumes <local time>`, a toast SHALL announce when the wait starts and when it resumes or is abandoned, and the interrupt key SHALL cancel the wait as it interrupts a Turn.

#### Scenario: Schedule a resume
- **WHEN** a Turn fails with `usage limit reached; resets at 14:20` and the user runs `/resume-at-reset`
- **THEN** the footer shows `⏸ usage limit · resumes 14:2x` and the Turn runs again after the reset without further input

#### Scenario: Nothing to resume
- **WHEN** the user runs `/resume-at-reset` after a Turn that ended normally
- **THEN** a toast shows the server's refusal and nothing is scheduled
