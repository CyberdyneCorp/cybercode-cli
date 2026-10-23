## MODIFIED Requirements

### Requirement: Stats command
(P0) `cyber stats [--days N] [--project <id>] [--models] [--tools] [--by origin] [--format json]` SHALL aggregate stored Sessions into totals of sessions, turns, tokens (input, output, reasoning, cache read, cache write), cost, tool usage, and per-model and per-workflow usage. (P1) Sessions, Turns, tokens, cost, streaks and the per-model table SHALL come from `GET /api/v1/usage/stats` of the shared server (`--days N` as `period=Nd`, 1 to 371; `--project <dir>` as `directory`, the project containing `<dir>`), so they match the TUI Stats tab. `--tools` SHALL count tool calls by name from the `tool_call` projection, for Sessions in scope that were active since the period's start. `--format json` SHALL print that response's `data` unchanged, plus `tools` when `--tools` is given. `--by origin` SHALL add one row per usage origin (`cross-session-messaging` → Usage attribution).

#### Scenario: Last seven days
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals include only Turns from the last 7 local days, with one row per model

#### Scenario: CLI and TUI agree
- **WHEN** the user runs `cyber stats --days 30 --format json` and opens the TUI Stats tab with `Last 30 days`
- **THEN** both show the same total tokens, Sessions and current streak

#### Scenario: Usage by origin
- **WHEN** the user runs `cyber stats --by origin` after prompting one Session and messaging another
- **THEN** separate rows for `user` and `session` show their turns, tokens and cost
