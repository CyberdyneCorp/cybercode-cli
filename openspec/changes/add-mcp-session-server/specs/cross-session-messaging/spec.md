## MODIFIED Requirements

### Requirement: Usage attribution
(P1) Tokens spent processing a delivered message SHALL be attributed to the receiving Session, tagged `origin: session`, so cost reports can separate user-driven usage from message-driven usage. Usage of runs started by MCP input (a prompt or message admitted through MCP serve, `mcp-server`) SHALL be tagged `origin: mcp` with `mcp_client`; a message sent through MCP keeps its Messaging API origin for inbound rules.

#### Scenario: Cost report
- **WHEN** the user runs `cyber stats --by origin`
- **THEN** message-driven usage is shown separately from user prompts

#### Scenario: MCP-driven usage
- **WHEN** an MCP client labelled `claude-code` prompts a Session and the run spends 3 Turns
- **THEN** the `usage.recorded.1` records of those Turns carry `origin: mcp` and `mcp_client: claude-code`
