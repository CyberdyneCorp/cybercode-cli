## ADDED Requirements

### Requirement: Usage limit wait notice
(P1) While the attached Session's `usage_limit` is set, the dashboard SHALL show a status notice `Usage limit reached. Resuming at <local time>; Interrupt cancels the wait.`, and the Interrupt control SHALL stay available because the Session is running. The notice SHALL disappear when the Session view no longer carries the wait.

#### Scenario: Waiting Session in the dashboard
- **WHEN** the browser attaches to a Session that waits for a usage limit to reset at 02:40
- **THEN** the conversation shows the notice with the resume time and the Interrupt button is enabled
