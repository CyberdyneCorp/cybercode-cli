## MODIFIED Requirements

### Requirement: powershell tool
(P1) On Windows, `powershell` SHALL run commands natively in `pwsh`, falling back to `powershell.exe`, with the same parameters, limits, sandboxing and permission analysis as `bash`. The permission action SHALL be `bash` so a single ruleset covers both.

#### Scenario: PowerShell on Windows
- **WHEN** the server runs on Windows and `pwsh` is installed
- **THEN** `powershell` is advertised and its calls are checked against `bash` rules

#### Scenario: Native PowerShell permission resources
- **WHEN** native PowerShell source contains multiple simple commands, literal filesystem operands or output redirections
- **THEN** permission resources SHALL come from the bounded PowerShell AST rather than a Bash parse
- **AND** ordinary quoted output SHALL remain data
- **AND** literal filesystem targets SHALL participate in external-directory and protected-path checks
- **AND** unresolved bindings or command contexts SHALL NOT enable classifier approval

#### Scenario: Native PowerShell foreground lifecycle
- **WHEN** an approved native PowerShell call times out or its Turn is interrupted
- **THEN** its parent-owned process boundary SHALL terminate the interpreter and settle the call
- **AND** closed standard input SHALL remain closed to the user command
- **AND** unavailable Windows confinement or a missing process-owner helper SHALL refuse launch

#### Scenario: Native PowerShell discovery and modes
- **WHEN** Windows tool definitions are requested
- **THEN** interpreter discovery SHALL prefer an installed absolute `pwsh.exe` before `powershell.exe` and exclude checkout executables and relative PATH entries
- **AND** plan mode or a `bash` tool deny SHALL hide `powershell`
- **AND** a denied constituent command SHALL refuse compound source execution

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

This list and the built-in set above are the single registry of model-facing tool names; a capability SHALL NOT introduce a tool absent from them. All of these SHALL use the shared tool-registry contract: schema validation, output budget, permission assertion with the tool name as the action unless the owning spec states otherwise, and hiding when fully denied. A tool whose owning capability or phase is not implemented in a build SHALL be absent from the registry rather than advertised as a stub, so the advertised tools are always a subset of this list and a call to an absent tool settles as `Unknown tool: <name>`.

#### Scenario: Phase-gated tool hidden
- **WHEN** the agent-teams capability is disabled (feature `teams` off)
- **THEN** `team_spawn`, `team_merge` and the `task_*` tools are not advertised to the model, and a call to them settles as `Unknown tool: team_spawn`

#### Scenario: Cross-capability tool obeys permissions
- **WHEN** config denies `send_message` for resource `*` in agent `explore`
- **THEN** `send_message`, `list_sessions` and `watch_session` remain subject to their own rules, and `send_message` is omitted from the `explore` agent's advertised tools

#### Scenario: Unimplemented phase is absent
- **WHEN** a build does not implement a listed tool's phase, such as the PTY tools before PTY sessions exist
- **THEN** the tool SHALL NOT be advertised in any mode, and every advertised name SHALL appear in this registry

### Requirement: monitor tool
(P1) The `monitor` tool's parameters, delivery and limits are specified by `background-tasks` (Monitor tool). It SHALL check the `bash` permission for command and file sources and the `network` permission for URL sources, and SHALL return the `job_` ID.

#### Scenario: React to a log line
- **WHEN** the model monitors `tail -F app.log` with filter `ERROR`
- **THEN** each new `ERROR` line is admitted into the Session as a queued message, at most 30 per minute

#### Scenario: Source permissions fail closed
- **WHEN** the model monitors a command
- **THEN** the command SHALL be authorized and sandboxed as a `bash` command, and the sandbox proxy's network questions from it SHALL be refused
- **WHEN** the model monitors a file
- **THEN** the `bash` permission SHALL be checked for `tail -F <path>`, together with the external-directory check and the sandbox profile's `deny_read`
- **WHEN** the model monitors a URL
- **THEN** a domain the sandbox denies SHALL be refused, and otherwise the `network` permission for the host SHALL be asked even in `bypass`
- **AND** `monitor` SHALL be hidden when `bash` is fully denied
