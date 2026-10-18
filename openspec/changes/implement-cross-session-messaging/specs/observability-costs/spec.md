## MODIFIED Requirements

### Requirement: Usage commands
(P1) The system SHALL provide `/cost` (session and descendants: tokens by class, cost, cache hit rate), `/status` (model, mode, agent, context window use, sandbox state, account, MCP and LSP summary), and `/usage` (today, last 7 and 30 days for this machine). It SHALL also provide `cyber stats [--days N] [--models] [--tools] [--project <id>] [--plugins] [--by origin] [--format json]`, where `--by origin` groups usage by the origin recorded on each step (`cross-session-messaging` "Usage attribution").

#### Scenario: Stats for a week
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals of sessions, messages, tokens and cost, plus a per-model table, are printed for the last 7 days

#### Scenario: Message-driven usage separated
- **WHEN** the user runs `cyber stats --by origin --format json`
- **THEN** the output has one entry per origin, such as `user`, `session`, `mcp` and `workflow`, each with tokens and cost
