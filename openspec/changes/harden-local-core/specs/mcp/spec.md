## MODIFIED Requirements

### Requirement: Server configuration
(P0) The system SHALL read MCP servers from the `mcp` config key, keyed by server name (`^[a-zA-Z0-9_-]{1,48}$`). Local entries SHALL be `{ type: "local", command, args?, env?, cwd?, enabled?, timeout?, tools? }` and remote entries `{ type: "remote", url, headers?, oauth?, enabled?, timeout?, tools? }`, where `tools` is `{ allow?: [glob], deny?: [glob] }`. `env` and `headers` values SHALL support `{env:NAME}` substitution.

#### Scenario: Local server
- **WHEN** config has `"mcp": { "fs": { "type": "local", "command": "npx", "args": ["-y", "@mcp/fs"] } }`
- **THEN** the server is spawned over stdio when the Location opens only after workspace-trust approves any project-controlled definition

#### Scenario: Tool deny filter
- **WHEN** a server entry sets `tools.deny: ["delete_*"]`
- **THEN** tools whose names match `delete_*` are not registered
