## MODIFIED Requirements

### Requirement: Tool search and deferred loading
(P1) When the materialized tool definitions exceed `tool_output.deferred_threshold_tokens` (defined by `tool-registry`), the system SHALL defer MCP tool definitions. In that case it SHALL expose only their names and one-line descriptions plus the `tool_search` tool that returns matching full definitions and loads them into the following Turns.

#### Scenario: Many tools deferred
- **WHEN** 300 MCP tools exceed the threshold
- **THEN** the model sees a name list and calls `tool_search` with `query: "jira"` to load the Jira tools
