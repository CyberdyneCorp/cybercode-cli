## MODIFIED Requirements

### Requirement: Deferred tools and tool search
(P1) When the materialized tool definitions exceed `tool_output.deferred_threshold_tokens` (default 10000 estimated tokens, 4 characters per token), the system SHALL advertise MCP and plugin tools as deferred: name and one-line description only, without parameter schemas. This key is the only deferral threshold; the `mcp` capability SHALL NOT define a separate one. It SHALL also expose a `tool_search` tool that takes `{ query?: string, select?: string[], limit?: number (default 5) }` and returns full schemas, which makes the selected tools callable for the rest of the Session.

#### Scenario: Large MCP catalog deferred
- **WHEN** connected MCP servers contribute 300 tools totaling 60000 schema tokens
- **THEN** the model sees deferred names plus `tool_search`
- **AND** calling a deferred tool before loading it returns `Tool <name> is deferred. Load it with tool_search first.`

#### Scenario: Selecting a deferred tool
- **WHEN** the model calls `tool_search` with `select: ["mcp__github__create_issue"]`
- **THEN** the result contains that tool's full schema and the tool is advertised with parameters on subsequent Turns

### Requirement: Managed tool output files
(P0) The system SHALL write the complete text of every truncated output to an exclusively created file `<data>/tool-output/tool_<ulid>` and record the path in the call's `output_paths` metadata. Cleanup of unpinned files older than 7 days SHALL be performed by the storage GC (`storage-events` → Retention and garbage collection), which preserves references required by active Sessions, workflows, recovery records and backups. Failure to write the overflow file SHALL fail the call operationally instead of returning lossy output.

#### Scenario: Overflow file readable later
- **WHEN** a truncated call recorded `<data>/tool-output/tool_01J...`
- **THEN** the model can `read` that path without an `external_directory` prompt
