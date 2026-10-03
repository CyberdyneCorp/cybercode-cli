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
(P0) Without configuration the system SHALL apply: `* ask`; `read`, `glob`, `grep` and `list` allow within the Location; `edit`, `bash` and external mutations ask; `external_directory ask` except the tool-output, jobs, temp, skill and worktree directories; `read` and `edit` `ask` for `*.env` and `*.env.*` but `allow` for `*.env.example`; `question`, `plan_enter` and `plan_exit` allow for primary agents (`mode` `primary` or `all`) and deny for subagent-only and hidden agents; `doom_loop ask`; `message.send ask` for cross-machine targets; `workflow.run ask`; `remote.attach ask`.

#### Scenario: Reading .env asks
- **WHEN** the model reads `.env.local` with default rules
- **THEN** a permission request is raised

#### Scenario: Subagent cannot ask questions
- **WHEN** an `explore` subagent calls `question`
- **THEN** the call is denied and the subagent is told to return its best answer instead

### Requirement: Protected paths
(P0) The system SHALL treat these as protected for mutation: `.git/` (the repository metadata directory, not the working tree); the configuration documents `cyber.json`, `cyber.jsonc`, `.cyber/cyber.jsonc`, `.cyber/cyber.local.jsonc`, `.cyber/hooks.jsonc`, `.cyber/mcp.json`, `.cyber/plugins.json`, `.cyber/plugins.local.json`, `.cyber/plugins.lock` and `.cyber/plugins/`; shell startup files (`~/.bashrc`, `~/.zshrc`, `~/.profile`, `~/.config/fish/config.fish`); `~/.ssh/`, `~/.gnupg/`, `~/.aws/credentials` and `~/.config/cyber/`. The `.cyber/` content directories `plans/`, `workflows/`, `agents/`, `skills/`, `commands/`, `teams/`, `output-styles/` and files `.cyber/import-report-*.md` SHALL NOT be protected; they follow ordinary `edit` rules so the system can write plans, saved workflows, generated agents and recorded skills. Mutating a protected path SHALL require `ask` in every Mode, including `bypass` and `auto`, unless an explicit config rule names the exact path with `allow`. The `sandbox` capability SHALL enforce the same partition for OS-level writes.

#### Scenario: Bypass still asks for .ssh
- **WHEN** a Session in `bypass` mode attempts to write `~/.ssh/config`
- **THEN** a permission request is raised

#### Scenario: Plan file is ordinary content
- **WHEN** a `plan`-mode Session writes `.cyber/plans/auth-refactor.md`
- **THEN** no protected-path prompt is raised and the write succeeds under plan-mode rules

#### Scenario: Hook definition stays protected
- **WHEN** an `accept-edits` Session edits `.cyber/hooks.jsonc`
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
(P0) The system SHALL support Modes `default` and `plan` in P0, adding `accept-edits`, `auto`, `dont-ask` and `bypass` in P1, selectable per Session (`--mode`, `mode` config, agent `permission_mode`, `/mode <name>`, `POST /api/v1/sessions/:id/mode`). The TUI SHALL cycle Modes with Shift+Tab through `default → accept-edits → plan → auto → default` on P1 builds (`default → plan → default` on P0 builds); `bypass` and `dont-ask` SHALL be reachable only through `/mode`, flags or config. A Mode change SHALL apply at the next Turn and publish `session.mode.switched.1`, as specified by session-runtime. The UI SHALL show a requested change as pending until effective; interrupt SHALL be offered when immediate cancellation is needed.

#### Scenario: Cycling modes
- **WHEN** the user presses Shift+Tab twice from `default` on a P1 build
- **THEN** the Session mode becomes `plan`

### Requirement: accept-edits mode
(P1) In `accept-edits`, `edit`, `write`, `apply_patch` and `notebook_edit` inside the Location, and the filesystem commands `mkdir`, `touch`, `mv`, `cp` and `rm` (non-recursive) on paths inside the Location, SHALL evaluate as `allow` unless a rule denies them. Every other `ask` stays `ask`.

#### Scenario: Edit auto-approved
- **WHEN** a Session in `accept-edits` edits `src/main.rs`
- **THEN** no prompt is shown

### Requirement: plan mode
(P0) In `plan`, the system SHALL allow only tools annotated `read_only`, plus writing the plan file `<location>/.cyber/plans/<session-slug>.md`, and SHALL deny every other mutating action with `Plan mode is read-only. Present the plan with plan_exit.` The system SHALL provide the tools `plan_enter {}` (switch the Session to `plan` at the next Turn) and `plan_exit { summary }` (present the plan file for approval). `plan_exit` SHALL route a question to the user with the choices `Approve and build (default)`, `Approve with accept-edits`, `Approve with auto` (only when `auto` is available) and `Request changes`. An approval SHALL switch the Mode to the chosen one at the next Turn; `Request changes` SHALL return the user's feedback as the tool result and keep `plan`. In non-interactive Sessions both tools SHALL be denied. Planning is a Mode, not an agent: there is no built-in `plan` agent.

#### Scenario: Write blocked in plan mode
- **WHEN** a `plan` Session calls `edit` on `src/lib.rs`
- **THEN** the call is denied with `Plan mode is read-only. Present the plan with plan_exit.`

#### Scenario: Plan approved with accept-edits
- **WHEN** the model calls `plan_exit` and the user chooses `Approve with accept-edits`
- **THEN** the Session's Mode becomes `accept-edits` at the next Turn and `session.mode.switched.1` is published

#### Scenario: Changes requested
- **WHEN** the user chooses `Request changes` and types `split the migration into two steps`
- **THEN** the tool returns that feedback to the model and the Session stays in `plan`

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

### Requirement: Session ruleset API
(P0) The Session ruleset layer SHALL be settable by clients: `PUT /api/v1/sessions/:id/permissions/rules` with `{ rules: [{ action, resource, effect }] }` SHALL replace the Session's rules, `GET` SHALL return them with the effective merged ruleset, and the SDK SHALL expose `session.permissions.set(rules)` and `.get()`. `cyber exec --allow <action[:resource]>` and `--deny <action[:resource]>` SHALL populate this layer for the new Session. Session rules SHALL be recorded as a durable `session.permissions.updated.1` event and apply at the next Safe Boundary. They SHALL NOT widen explicit user/global denies, plan-mode restrictions, protected paths, workspace trust, sandbox boundaries or org policy, which remain ceilings.

#### Scenario: Script narrows a session
- **WHEN** an SDK client calls `session.permissions.set([{ action: "bash", resource: "*", effect: "deny" }])` on a running Session
- **THEN** from the next Safe Boundary every `bash` call is denied and the event is recorded

#### Scenario: Session rule cannot beat an org deny
- **WHEN** a Session rule allows `webfetch` for `*` and org policy denies `webfetch` for `*.internal.example.com`
- **THEN** fetching an internal host is still denied with reason `org policy`

### Requirement: Auto-mode configuration and override
(P1) `permissions.auto_mode` SHALL accept `rules.always_block` and `rules.always_allow` (ordered `{ action, resource }` patterns applied before the classifier), `policy` (text appended to the classifier prompt describing the user's boundaries), `classify_read_only` (default false: read-only tools skip the classifier) and `fallback` (`ask` default, or `deny`). `/approve` SHALL re-issue the most recent call blocked by the classifier once, after the user confirms it in a permission prompt showing the classifier's reason; `cyber permissions auto show|reset` SHALL print or clear learned per-checkout statistics. Org policy MAY set the same keys as ceilings (`always_block` unions, `always_allow` intersects).

#### Scenario: Always block a deploy command
- **WHEN** `permissions.auto_mode.rules.always_block` contains `{ action: "bash", resource: "kubectl apply *" }`
- **THEN** that command is blocked in `auto` without consulting the classifier

#### Scenario: Override one block
- **WHEN** the classifier blocked `git push origin feature/x` and the user runs `/approve`
- **THEN** the user sees the command and the classifier's reason, and on confirmation the push runs once

### Requirement: Rule dry run
(P1) `cyber permissions test [--agent <a>] [--mode <m>] (--tool <name> --resource <r> | -- <shell command>)` SHALL evaluate the effective ruleset exactly as a Session would (including bash command splitting, protected paths, saved approvals, Session Mode and org policy) and print the resulting effect, the matching rule and its source layer for each evaluated resource, without creating a Session or running anything. `GET /api/v1/permissions/test` SHALL expose the same evaluation.

#### Scenario: Explain why a command would prompt
- **WHEN** the user runs `cyber permissions test -- "npm test && git push"`
- **THEN** the output lists `npm test → allow (saved approval, project)` and `git push → ask (default rules)`

### Requirement: Additional working directories
(P0) `--add-dir <path>` (repeatable), `/add-dir <path>` and `POST /api/v1/sessions/:id/directories` SHALL add a directory to the Session: it SHALL be allowed for `external_directory` reads and edits under ordinary tool rules, added to the sandbox writable roots, and its instruction files loaded as a Context Source. Additions SHALL be recorded as durable `session.directory.added.1` events, fire the `DirectoryAdded` hook, and apply at the next Safe Boundary. Protected paths inside an added directory stay protected.

#### Scenario: Shared library next to the repo
- **WHEN** the user runs `cyber --add-dir ../shared-lib`
- **THEN** reads and edits under `../shared-lib` need no external-directory prompt and its `AGENTS.md` is in the context
