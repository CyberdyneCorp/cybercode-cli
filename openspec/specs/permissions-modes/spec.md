# permissions-modes Specification

## Purpose
Decides, for every tool action, whether it runs automatically (`allow`), needs approval (`ask`) or is blocked (`deny`), and how that decision changes under a permission Mode. The rule engine is the one OpenCode v1 and v2 share (ordered `{action, resource, effect}` rules, wildcard matching, last match wins). The Modes are Claude Code's set (`accept-edits`, `plan`, `auto` with a classifier, `dont-ask`, `bypass`), plus Codex-style approval ergonomics and OpenCode v2's persisted approvals. Together they make unattended, looped and workflow work safe.

## Requirements

### Requirement: Rule model
(P0) The system SHALL model permissions as ordered rules `{ action, resource, effect }` with `effect` one of `allow`, `ask`, `deny`. In config it SHALL accept `permissions` as either a string shorthand (`"ask"` ≡ `{ "*": "ask" }`) or a map `action → effect | { pattern → effect }`, preserving the written key order. Actions SHALL be tool names plus `external_directory`, `network`, `message.send`, `workflow.run`, `goal.set`, `remote.attach`, `sandbox.escalate`, `doom_loop`.

#### Scenario: Map shorthand expanded in order
- **WHEN** config has `"permissions": { "bash": { "*": "ask", "git status": "allow" } }`
- **THEN** the rules are `bash/* ask` followed by `bash/git status allow`

### Requirement: Wildcard matching
(P0) Action and resource patterns SHALL match whole strings, where `*` matches any sequence including `/` and newlines and `?` matches one character. Backslashes SHALL be normalized to `/`. A pattern ending in ` *` SHALL also match the bare prefix. Matching SHALL be case-insensitive on Windows only. Resources for path tools starting with `~` or `$HOME` SHALL be expanded to the home directory.

#### Scenario: Trailing wildcard matches bare command
- **WHEN** the rule resource is `git log *` and the command is `git log`
- **THEN** the rule matches

### Requirement: Last matching rule wins
(P0) The system SHALL evaluate a request against the effective ruleset and use the last rule whose action and resource both match. It SHALL default to `ask` when none match. Pattern specificity SHALL NOT affect precedence. For a request naming several resources, the result SHALL be `deny` if any is `deny`, else `ask` if any is `ask`, else `allow`.

#### Scenario: Later broad rule overrides earlier specific rule
- **WHEN** rules are `edit/src/** allow` then `edit/* ask`
- **THEN** editing `src/a.rs` evaluates to `ask`

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

### Requirement: Protected paths
(P0) The system SHALL treat `.git/`, `.cyber/`, `cyber.json(c)`, shell startup files (`~/.bashrc`, `~/.zshrc`, `~/.profile`, `~/.config/fish/config.fish`), `~/.ssh/`, `~/.gnupg/`, `~/.aws/credentials` and `~/.config/cyber/` as protected for mutation. Mutating a protected path SHALL require `ask` in every Mode, including `bypass` and `auto`, unless an explicit config rule names the exact path with `allow`.

#### Scenario: Bypass still asks for .ssh
- **WHEN** a Session in `bypass` mode attempts to write `~/.ssh/config`
- **THEN** a permission request is raised

### Requirement: Critical-path removal guard
(P1) The system SHALL detect removals targeting critical paths (`/`, `~`, the home directory, the repository root, the Location root, `.git`, and any ancestor of the Location). These include `rm -rf`, `Remove-Item -Recurse`, `find -delete`, and removals inside nested shells or inline scripts. Such removals SHALL be denied in `auto`, `dont-ask` and `bypass` modes and SHALL require `ask` with a red warning in other modes. The model SHALL receive `Refused: removal of critical path <path>. Rewrite the command to target specific files.`

#### Scenario: rm -rf on repo root under bypass
- **WHEN** a `bypass` Session runs `rm -rf "$(git rev-parse --show-toplevel)"`
- **THEN** the command is refused without execution

### Requirement: Replies and approvals
(P0) A pending request SHALL accept replies `once`, `always` or `reject` with optional `message`. `reject` without a message SHALL decline and halt the current Drain. `reject` with a message SHALL return the feedback to the model as a tool error and continue. Any `reject` SHALL also decline every other pending request of the same Session. `always` SHALL approve and then auto-approve other pending requests that become fully allowed.

#### Scenario: Reject with feedback
- **WHEN** the user rejects an edit with message "use the existing helper"
- **THEN** the model receives the tool error `Rejected by user: use the existing helper` and the Drain continues

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

### Requirement: accept-edits mode
(P1) In `accept-edits`, `edit`, `write`, `apply_patch` and `notebook_edit` inside the Location, and the filesystem commands `mkdir`, `touch`, `mv`, `cp` and `rm` (non-recursive) on paths inside the Location, SHALL evaluate as `allow` unless a rule denies them. Every other `ask` stays `ask`.

#### Scenario: Edit auto-approved
- **WHEN** a Session in `accept-edits` edits `src/main.rs`
- **THEN** no prompt is shown

### Requirement: plan mode
(P0) In `plan`, the system SHALL allow only tools annotated `read_only`, plus writing the plan file `<location>/.cyber/plans/<session-slug>.md`, and SHALL deny every other mutating action. Exiting plan mode SHALL go through `plan_exit`, which presents the plan for approval. Approval SHALL switch the Session to the Mode chosen by the user (`default`, `accept-edits` or `auto`).

#### Scenario: Write blocked in plan mode
- **WHEN** a `plan` Session calls `edit` on `src/lib.rs`
- **THEN** the call is denied with `Plan mode is read-only. Present the plan with plan_exit.`

### Requirement: auto mode classifier
(P1) In `auto`, every request that would be `ask` SHALL be reviewed by a classifier using `model_roles.evaluator`, else `small_model`. The classifier SHALL receive the tool call, the last 20 messages, the Location and the user's stated boundaries. It SHALL return `allow` or `block` with a reason. The system SHALL block irreversible or out-of-scope actions (force pushes, deploys, deletes outside the Location, credential access, data exfiltration to non-allowlisted hosts). After 3 consecutive blocks in one Drain, or when the classifier is unavailable, the system SHALL fall back to `ask`, or to `deny` when no user is attached. Each decision SHALL be recorded as `permission.auto_decided.1` with the reason.

#### Scenario: Force push blocked
- **WHEN** an `auto` Session runs `git push --force origin main`
- **THEN** the classifier blocks it and the model receives `Blocked by auto mode: <reason>`

#### Scenario: Fallback after repeated blocks
- **WHEN** the classifier blocks 3 calls in a row
- **THEN** the next ask is shown to the user as a normal prompt

### Requirement: dont-ask mode
(P1) In `dont-ask`, every evaluation resulting in `ask` SHALL become `deny` with the model-visible message `Not pre-approved (dont-ask mode)`, and the Drain SHALL continue. This is the default Mode for `cyber exec` without a TTY.

#### Scenario: CI run denies unapproved command
- **WHEN** `cyber exec` without a TTY needs approval for `curl`
- **THEN** the call is denied and the run continues

### Requirement: bypass mode
(P1) `bypass` SHALL evaluate every request as `allow` except explicit user/global deny ceilings, protected paths, critical-path removals, workspace trust and org policy denies. It SHALL be enabled only when the process runs inside the OS sandbox with `workspace-write` or stricter, or inside a detected container or VM, or when `--dangerously-bypass-permissions` is passed explicitly. Enabling it outside these conditions SHALL fail with `bypass mode requires a sandbox, a container, or --dangerously-bypass-permissions`.

#### Scenario: Bypass refused on bare host
- **WHEN** the user sets `mode: "bypass"` with sandbox `full-access` on a host machine and no flag
- **THEN** the Session refuses to start in bypass and reports the requirement

### Requirement: Doom-loop detection
(P0) When the last 3 tool calls of an assistant message are the same tool with byte-identical input, the system SHALL raise a `doom_loop` permission request with the tool name as resource before the next call. In `auto` and `dont-ask` it SHALL halt the Drain with `Repeated identical tool calls detected`.

#### Scenario: Identical calls interrupted
- **WHEN** the model calls `read src/a.rs` with the same arguments three times in a row
- **THEN** a `doom_loop` request is raised before the fourth call

### Requirement: Permission requests, routes and events
(P0) Pending requests SHALL carry `{ id: per_..., session_id, action, resources, always_patterns, metadata (diff, command), tool: { message_id, call_id } }`. They SHALL be held per Location and declined when the Location shuts down. The server SHALL expose `GET /api/v1/permissions/requests`, `POST /api/v1/sessions/:id/permissions/:request_id/reply` `{ reply, message? }`, and the events `permission.asked.1` and `permission.replied.1`.

#### Scenario: Remote client answers a prompt
- **WHEN** a mobile client posts `reply: "once"` for a pending request
- **THEN** the tool proceeds and `permission.replied.1` is published to all attached clients

### Requirement: Non-interactive behavior
(P0) Sessions without any attached interactive client (exec, routines, workflow agents, channels) SHALL resolve `ask` according to their Mode: `auto` uses the classifier, `dont-ask` denies, and `default` denies with a printed warning. They SHALL never block indefinitely. Interactive clients attaching later SHALL see pending requests.

#### Scenario: Routine in default mode
- **WHEN** a routine running in `default` mode needs approval
- **THEN** the request is denied, logged, and the run continues

### Requirement: Subagent permission inheritance
(P2) A subagent Session SHALL inherit the parent's `deny` rules, `external_directory` rules, org policy and Mode ceiling (a child SHALL NOT run in a more permissive Mode than its parent unless the workflow or agent definition sets it explicitly and the parent Mode is `auto` or `bypass`). Prompts from subagents SHALL be surfaced in the parent's client labeled with the subagent name.

#### Scenario: Child cannot escalate mode
- **WHEN** a `default`-mode Session spawns a subagent whose definition says `mode: bypass`
- **THEN** the subagent runs in `default`
