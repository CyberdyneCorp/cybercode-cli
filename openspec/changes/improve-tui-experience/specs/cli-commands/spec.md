## MODIFIED Requirements

### Requirement: Stats command
(P0) `cyber stats [--days N] [--project <id>] [--models] [--tools] [--format json]` SHALL aggregate stored Sessions into totals of sessions, turns, tokens (input, output, reasoning, cache read, cache write), cost, tool usage, and per-model and per-workflow usage. (P1) Sessions, Turns, tokens, cost, streaks and the per-model table SHALL come from `GET /api/v1/usage/stats` (`--days N` as `period=Nd`, `--project` as its project scope), so they match the TUI Stats tab. `--format json` SHALL print that response unchanged, plus tool usage when `--tools` is given.

#### Scenario: Last seven days
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals include only Turns from the last 7 local days, with one row per model

#### Scenario: CLI and TUI agree
- **WHEN** the user runs `cyber stats --days 30 --format json` and opens the TUI Stats tab with `Last 30 days`
- **THEN** both show the same total tokens, Sessions and current streak
