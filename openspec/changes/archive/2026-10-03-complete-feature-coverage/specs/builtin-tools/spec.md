## MODIFIED Requirements

### Requirement: websearch tool
(P0) `websearch` SHALL accept `{ query, max_results? (default 8, max 20), allowed_domains?, blocked_domains? }` and check the `websearch` permission on the query. It SHALL use the model provider's native search when the Turn model's catalog entry declares `capabilities.native_web_search`, and otherwise the configured backend `tools.websearch.backend` (`exa`, `brave`, `searxng`, `parallel`, `firecrawl`, `tavily`, `tinyfish`, or `random`, which rotates among backends with stored credentials and skips a backend for 10 minutes after it returns 429), with credentials from `provider-credentials`. `tools.websearch.enabled: false` SHALL hide the tool. Requests SHALL time out after 25 s and responses SHALL be limited to 256 KiB. Results SHALL be returned as `title`, `url`, `snippet`. Without any backend, the tool SHALL be hidden.

#### Scenario: Native search preferred
- **WHEN** the Turn model declares `native_web_search`
- **THEN** the search is executed by the provider and no third-party backend is contacted

#### Scenario: Rotation after rate limit
- **WHEN** `backend` is `random`, `exa` returns 429 and `brave` has credentials
- **THEN** the query is retried on `brave` and `exa` is skipped for the next 10 minutes

### Requirement: Capability-owned tool catalog
(P1) Besides the tools above, the registry SHALL register the following model-facing tools. Each is specified by the capability in parentheses and is advertised only when that capability is enabled and its phase is implemented:
- `agent`, `return_result` (agents-subagents)
- `workflow` (workflows)
- `schedule_wakeup`, `cron_create`, `cron_list`, `cron_delete` (loops-scheduling)
- `monitor`, `task_stop`, `pty_start`, `pty_write`, `pty_read`, `notify`, `send_file` (background-tasks)
- `enter_worktree`, `exit_worktree` (worktrees)
- `team_spawn`, `team_merge`, `task_create`, `task_update`, `task_list`, `task_get` (agent-teams)
- `list_sessions`, `send_message`, `watch_session` (cross-session-messaging)
- `channel_reply` (channels)
- `memory` (memory)
- `wait_for_mcp`, `mcp_list_resources`, `mcp_list_resource_templates`, `mcp_read_resource` (mcp)
- `publish_artifact` (session-sharing)
- `advisor` (provider-catalog)

This list and the built-in set above are the single registry of model-facing tool names; a capability SHALL NOT introduce a tool absent from them. All of these SHALL use the shared tool-registry contract: schema validation, output budget, permission assertion with the tool name as the action unless the owning spec states otherwise, and hiding when fully denied.

#### Scenario: Phase-gated tool hidden
- **WHEN** the agent-teams capability is disabled (feature `teams` off)
- **THEN** `team_spawn`, `team_merge` and the `task_*` tools are not advertised to the model, and a call to them settles as `Unknown tool: team_spawn`

#### Scenario: Cross-capability tool obeys permissions
- **WHEN** config denies `send_message` for resource `*` in agent `explore`
- **THEN** `send_message`, `list_sessions` and `watch_session` remain subject to their own rules, and `send_message` is omitted from the `explore` agent's advertised tools
