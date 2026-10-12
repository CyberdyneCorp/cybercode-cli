## MODIFIED Requirements

### Requirement: MCP server mode
(P1) The system SHALL provide `cyber mcp serve`, exposing Cyber Code as an MCP server so external agents and IDEs can create, list, read, prompt and interrupt Sessions and exchange cross-session messages, as the `mcp-server` capability specifies. MCP clients SHALL be trusted local clients of the server API (local authentication, loopback or Unix socket only) and SHALL gain no permission beyond what the server API allows a local client; permission requests SHALL stay with human clients and the target Session's Mode.

#### Scenario: Another agent drives Cyber Code
- **WHEN** an external MCP client calls `cyber_prompt` with `{ directory: "/repo", prompt: "fix the failing test" }`
- **THEN** a Session is created in `/repo`, the prompt runs, and the final assistant text is returned as tool content with `status: "completed"`
