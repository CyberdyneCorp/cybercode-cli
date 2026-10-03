## MODIFIED Requirements

### Requirement: Built-in tool set
(P0) The system SHALL register these built-in tools in every Location: `read`, `write`, `edit`, `apply_patch`, `glob`, `grep`, `list`, `bash`, `webfetch`, `websearch`, `todo`, `question`, `skill`, `notebook_edit`, `history_search` (compaction), `plan_enter`, `plan_exit` (permissions-modes), the per-prompt `structured_output` tool (session-runtime), `tool_search` when deferral is active (tool-registry), and the hidden `invalid`. It SHALL add `powershell` on Windows, `lsp` when code intelligence is enabled (P1) and `advisor` when `model_roles.advisor` is configured (P2). `monitor` and the other background tools are specified by `background-tasks` and listed in the capability-owned catalog. The registry SHALL offer `apply_patch` instead of `edit`/`write` to models whose catalog entry sets `capabilities.prefers_apply_patch: true`, and `edit`/`write` to all other models.

#### Scenario: GPT-family model gets apply_patch
- **WHEN** the Turn model's catalog entry has `prefers_apply_patch: true`
- **THEN** the advertised tools include `apply_patch` and exclude `edit` and `write`

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
- **WHEN** the agent-teams capability is disabled (`experimental.teams` unset)
- **THEN** `team_spawn`, `team_merge` and the `task_*` tools are not advertised to the model, and a call to them settles as `Unknown tool: team_spawn`

#### Scenario: Cross-capability tool obeys permissions
- **WHEN** config denies `send_message` for resource `*` in agent `explore`
- **THEN** `send_message`, `list_sessions` and `watch_session` remain subject to their own rules, and `send_message` is omitted from the `explore` agent's advertised tools

### Requirement: skill tool
(P0) `skill` SHALL accept `{ name, arguments? }`, check the `skill` permission on the name, and return the skill body wrapped in `<skill name="..." base="...">` with a listing of up to 20 sibling files (relative paths), as specified by `skills-commands`. An unknown name SHALL fail with `Skill "<name>" not found. Available: <names>`.

#### Scenario: Load a skill
- **WHEN** the model calls `skill` with `name: "release-notes"`
- **THEN** the body of that skill's `SKILL.md`, its base directory and up to 20 sibling paths are returned

### Requirement: monitor tool
(P1) The `monitor` tool's parameters, delivery and limits are specified by `background-tasks` (Monitor tool). It SHALL check the `bash` permission for command and file sources and the `network` permission for URL sources, and SHALL return the `job_` ID.

#### Scenario: React to a log line
- **WHEN** the model monitors `tail -F app.log` with filter `ERROR`
- **THEN** each new `ERROR` line is admitted into the Session as a queued message, at most 30 per minute
