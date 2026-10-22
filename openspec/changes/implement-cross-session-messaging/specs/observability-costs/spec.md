## MODIFIED Requirements

### Requirement: Usage commands
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), `/usage` (the current Session tree's breakdown, configured Budgets, provider-reported limits and contributing factors for the last 24 hours or 7 days) and `/stats` (activity, streaks and per-model totals for all time, the last 7 days and the last 30 days). In the TUI these SHALL open the matching tab of the Settings panel. It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--by origin] [--project <id>] [--plugins] [--format json]`.

`--by origin` SHALL report the usage origins defined by `cross-session-messaging` → Usage attribution.

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

#### Scenario: Cost opens the Usage tab
- **WHEN** the user types `/cost` in the TUI
- **THEN** the Settings panel opens on the Usage tab at the Session section
