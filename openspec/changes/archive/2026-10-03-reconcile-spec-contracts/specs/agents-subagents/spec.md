## MODIFIED Requirements

### Requirement: Built-in agents
(P0) The system SHALL ship the built-in agents `build` (primary, full tool access), `explore` (subagent, read-only: `read`, `glob`, `grep`, `webfetch`, `websearch` and read-only `bash`), `general` (subagent, full tool access except `todo`), and the hidden system agents `compaction`, `title`, `summary` and `evaluator`, which SHALL have every tool denied. Planning SHALL be the `plan` permission Mode (`permissions-modes`), not an agent; an agent definition MAY set `permission_mode: plan` to start Sessions in that Mode.

#### Scenario: Default agent set
- **WHEN** a user lists agents in a project with no agent configuration
- **THEN** `build`, `explore` and `general` are listed and the hidden system agents are omitted

#### Scenario: System agents cannot call tools
- **WHEN** the `title` agent's model emits a tool call
- **THEN** the call is settled as an error without executing and no permission request is created

### Requirement: Agent definition fields
(P0) An agent definition SHALL accept `description`, `system`, `model` (`provider/model[#variant]`), `variant`, `mode` (`primary`, `subagent` or `all`), `permission_mode` (one of the six Modes), `tools` (`allow` and `deny` lists of tool names or globs), `permissions` (ordered rules), `request` (a provider request overlay of `headers` and `body`, for example `temperature` or `top_p`, layered as defined by `provider-catalog`), `steps` (positive integer), `color`, `hidden`, `isolation` (`none` or `worktree`), `background` (boolean default for spawns), `memory` (`none`, `project` or `user`), `skills` (names preloaded into context) and `mcp` (subset of configured MCP server names). Unknown fields SHALL be rejected by schema validation, naming the agent and field.

#### Scenario: Unknown field rejected
- **WHEN** an agent definition contains `temprature: 0.2`
- **THEN** config validation fails with an error naming the agent and the field `temprature`

#### Scenario: Tool deny list hides tools
- **WHEN** an agent sets `tools.deny: ["bash", "web*"]`
- **THEN** `bash`, `webfetch` and `websearch` are omitted from that agent's tool definitions for every Turn

#### Scenario: Request overlay applied
- **WHEN** an agent sets `request.body.temperature: 0.2`
- **THEN** its Turns send `temperature: 0.2` after provider, model and variant defaults

### Requirement: Custom agents from config
(P0) The system SHALL read agents from the `agents` key of `cyber.jsonc`/`cyber.json`, keyed by name. An entry matching an existing agent SHALL patch only the fields it sets; a new name SHALL create an agent with `mode: "all"`; `disabled: true` SHALL remove the agent, including built-ins other than the hidden system agents.

#### Scenario: Override built-in model
- **WHEN** config sets `agents.explore.model` to `ollama/qwen3-coder`
- **THEN** `explore` subagents use that model and keep their built-in read-only rules

#### Scenario: Disable a built-in agent
- **WHEN** config sets `agents.explore.disabled` to `true`
- **THEN** `explore` is absent from listings, `@` autocomplete and the `agent` tool catalogue

### Requirement: Agent step limit
(P0) When an agent sets `steps`, the system SHALL, on the Turn that reaches the limit, send the request with no tools and `tool_choice: none` plus an instruction to reply with a text summary of completed and remaining work; tool calls still emitted SHALL fail with `Tools are disabled after the maximum agent steps`. Promoting newly admitted user input SHALL reset the step count. Without `steps`, primary agents SHALL be unbounded except for goal and workflow budgets, and subagents SHALL default to 50 steps.

#### Scenario: Last step disables tools
- **WHEN** an agent with `steps: 5` reaches its 5th Turn in one Drain
- **THEN** that Turn is sent without tool definitions and the Drain ends after the model's text reply

#### Scenario: Subagent default limit
- **WHEN** a `general` subagent without `steps` completes its 50th Turn with tool calls pending
- **THEN** its next Turn has no tools and returns a summary to the parent
