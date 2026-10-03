## MODIFIED Requirements

### Requirement: Local review command
(P1) The system SHALL provide `/review [target]` and `cyber review [target]`. The target is uncommitted changes (default), `<base>..<head>`, a commit SHA, or `pr <number>`. In P1 the review SHALL run the bundled `review` skill (`skills-commands`) in a read-only `reviewer` subagent. From P2 it SHALL instead run the bundled `review` workflow (`workflows`), which checks correctness, security, tests and maintainability in parallel and verifies each finding before reporting it. In both cases results SHALL be reported as structured findings with `file`, `line`, `severity` (`critical`, `high`, `medium`, `low`), `summary` and `failure_scenario`.

#### Scenario: Review uncommitted changes
- **WHEN** the user runs `/review` with 4 modified files
- **THEN** the review runs over those 4 files and the TUI shows the findings, most severe first

#### Scenario: Unverified findings dropped
- **WHEN** (P2) the review workflow's verification agent rejects a reviewer finding
- **THEN** the finding is excluded from the final list and counted in `dropped`
