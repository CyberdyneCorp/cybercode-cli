## MODIFIED Requirements

### Requirement: Stats command
(P0) `cyber stats [--days N] [--project <id>] [--models] [--tools] [--by origin] [--format json]` SHALL aggregate stored Sessions into totals of sessions, turns, tokens (input, output, reasoning, cache read, cache write), cost, tool usage, and per-model and per-workflow usage. `--by origin` SHALL add one row per usage origin (`cross-session-messaging` "Usage attribution").

#### Scenario: Last seven days
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals include only Sessions updated in the last 7 days, with one row per model

#### Scenario: Usage by origin
- **WHEN** the user runs `cyber stats --by origin` after prompting one Session and messaging another
- **THEN** separate rows for `user` and `session` show their turns, tokens and cost
