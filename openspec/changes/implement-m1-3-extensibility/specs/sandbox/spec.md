## MODIFIED Requirements

### Requirement: Sandbox scope
(P0) The sandbox SHALL apply to `bash`, `powershell`, `monitor` commands and background tasks. With `sandbox.apply_to` (default `["tools", "hooks", "formatters"]`; add `"mcp"` to opt in) it SHALL also apply to hook commands, formatters and MCP stdio servers. Cyber's own server process SHALL never be sandboxed by this mechanism.

#### Scenario: MCP server sandboxed on opt-in
- **WHEN** `sandbox.apply_to` includes `mcp`
- **THEN** stdio MCP servers start under the sandbox with the Location as their only writable root

#### Scenario: Default scope excludes MCP
- **WHEN** `sandbox.apply_to` is absent
- **THEN** the sandbox scope SHALL NOT include MCP servers
- **AND** project server approval SHALL remain independent of sandbox scope

#### Scenario: MCP Location boundary
- **WHEN** an opted-in workspace-write MCP server starts at a nested Location
- **THEN** checkout siblings and configured additional workspace roots SHALL NOT become writable

#### Scenario: MCP proxy owns transport configuration
- **WHEN** an opted-in server uses proxy-only networking
- **THEN** owned allowlisted proxy requests SHALL succeed while direct upstream connections SHALL be refused
- **AND** configured proxy environment overrides SHALL NOT bypass the owned proxy
