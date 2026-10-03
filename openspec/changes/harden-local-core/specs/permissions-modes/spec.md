## MODIFIED Requirements

### Requirement: Ruleset layering
(P0) The effective ruleset SHALL be concatenated, lowest priority first, as: built-in defaults, built-in agent rules, global config, project config (farthest to nearest), the agent's own `permissions`, the Session ruleset, the active Mode's rules, then saved approvals as `allow` rules. Explicit user/global denies, plan-mode restrictions, protected-path checks, workspace trust and sandbox boundaries SHALL remain ceilings that saved approvals and project configuration cannot widen. Org policy rules (`org-policy`) SHALL be evaluated afterwards as ceilings: a policy `deny` SHALL be final and SHALL NOT be overridden by any layer, Mode or approval.

#### Scenario: Org deny beats local allow
- **WHEN** project config allows `webfetch` and org policy denies `webfetch` for `*.internal.example.com`
- **THEN** fetching `https://wiki.internal.example.com` is denied with reason `org policy`

### Requirement: Default rules
(P0) Without configuration the system SHALL apply: `* ask`; `read`, `glob`, `grep` and `list` allow within the Location; `edit`, `bash` and external mutations ask; `external_directory ask` except the tool-output, temp, skill and worktree directories; `read` and `edit` `ask` for `*.env` and `*.env.*` but `allow` for `*.env.example`; `question`, `plan_enter`, `plan_exit` deny except for the `build` and `plan` agents; `doom_loop ask`; `message.send ask` for cross-machine targets; `workflow.run ask`; `remote.attach ask`.

#### Scenario: Reading .env asks
- **WHEN** the model reads `.env.local` with default rules
- **THEN** a permission request is raised

### Requirement: Persisted approvals
(P0) An `always` reply SHALL persist its suggested patterns as saved approvals `{ project_id, checkout_root, action, resource, created_at, source_session }`, unique per checkout root, action and resource. Saved approvals SHALL apply to Sessions in that checkout across restarts, without transferring executable trust to other clones. They SHALL be listed by `cyber permissions list` and `GET /api/v1/permissions/saved`, and revoked by `cyber permissions revoke <id>` and `DELETE /api/v1/permissions/saved/:id`.

#### Scenario: Approval survives restart
- **WHEN** the user approves `bash: npm test *` with `always` and restarts the server
- **THEN** `npm test -- --watch` runs without prompting in any Session of that checkout

### Requirement: Permission modes
(P0) The system SHALL support Modes `default` and `plan` in P0, adding `accept-edits`, `auto`, `dont-ask` and `bypass` in P1, selectable per Session (`--mode`, `mode` config, agent `mode`, `POST /api/v1/sessions/:id/mode`) and cycled with Shift+Tab in the TUI through `default → accept-edits → plan → auto` (`bypass` and `dont-ask` only via flag or config); P0 cycles `default → plan` only. A Mode change SHALL apply at the next Turn and publish `session.mode.switched.1`, as specified by session-runtime. The UI SHALL show a requested change as pending until effective; interrupt SHALL be offered when immediate cancellation is needed.

#### Scenario: Cycling modes
- **WHEN** the user presses Shift+Tab twice from `default` on a P1 build
- **THEN** the Session mode becomes `plan`

### Requirement: plan mode
(P0) In `plan`, the system SHALL allow only tools annotated `read_only`, plus writing the plan file `<location>/.cyber/plans/<session-slug>.md`, and SHALL deny every other mutating action. Exiting plan mode SHALL go through `plan_exit`, which presents the plan for approval. Approval SHALL switch the Session to the Mode chosen by the user (`default`, `accept-edits` or `auto`).

#### Scenario: Write blocked in plan mode
- **WHEN** a `plan` Session calls `edit` on `src/lib.rs`
- **THEN** the call is denied with `Plan mode is read-only. Present the plan with plan_exit.`

### Requirement: bypass mode
(P1) `bypass` SHALL evaluate every request as `allow` except explicit user/global deny ceilings, protected paths, critical-path removals, workspace trust and org policy denies. It SHALL be enabled only when the process runs inside the OS sandbox with `workspace-write` or stricter, or inside a detected container or VM, or when `--dangerously-bypass-permissions` is passed explicitly. Enabling it outside these conditions SHALL fail with `bypass mode requires a sandbox, a container, or --dangerously-bypass-permissions`.

#### Scenario: Bypass refused on bare host
- **WHEN** the user sets `mode: "bypass"` with sandbox `full-access` on a host machine and no flag
- **THEN** the Session refuses to start in bypass and reports the requirement
