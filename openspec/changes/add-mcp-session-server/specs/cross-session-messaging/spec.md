## MODIFIED Requirements

### Requirement: Usage attribution
(P1) Tokens spent processing a delivered message SHALL be attributed to the receiving Session, tagged `origin: session`, so cost reports can separate user-driven usage from message-driven usage. Usage of runs started by MCP input (a prompt or message admitted through MCP serve, `mcp-server`) SHALL be tagged `origin: mcp` with `mcp_client`; a message sent through MCP keeps its Messaging API origin for inbound rules. Each provider step SHALL carry the origin of the most recently promoted input of its Session: `mcp` for MCP input, `session` for a message sent by another Session's agent (and, with "Direct peer transport", by a peer Session), `client` for a message sent through the Messaging API or `cyber messages send`, `job` for a background handback, `user` for user input, and otherwise the admission source (`workflow`, `hook`, `loop`, `goal`, `channel`). The origin SHALL be recorded on `session.step.started.1`, on the step's usage records and in the usage ledger; records made before origins existed SHALL read as `user`.

#### Scenario: Cost report
- **WHEN** the user runs `cyber stats --by origin`
- **THEN** message-driven usage is shown separately from user prompts

#### Scenario: MCP-driven usage
- **WHEN** an MCP client labelled `claude-code` prompts a Session and the run spends 3 Turns
- **THEN** the `usage.recorded.1` records of those Turns carry `origin: mcp` and `mcp_client: claude-code`

#### Scenario: Handback usage
- **WHEN** a background subagent's handback starts a Turn in its parent Session
- **THEN** that Turn's usage is tagged `origin: job`, not `session`

#### Scenario: Client message usage
- **WHEN** a script sends a message through `POST /api/v1/messaging/send` without `from` and the receiving Session spends one Turn on it
- **THEN** that Turn's usage is tagged `origin: client`
