# cli-commands Specification

## Purpose
Defines the `cyber` command-line surface: the command tree, global flags, environment markers, error format and exit codes. Scripts, CI, documentation and users depend on stable names and behavior. The design merges OpenCode v1's yargs tree and v2's `service`/`api` commands, Codex's `exec` ergonomics, and Claude Code's `agents`, `remote-control` and `--cloud`, `teleport` and `handoff` entry points. Each subsystem's internals are specified in its own capability; this spec only fixes the CLI contract.

## Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register exactly these top-level commands, grouped as shown by `--help`:
- **Core**: the default TUI command `cyber [project]`, `exec`, `web`, `attach`, `sessions`, `models`, `providers`, `agents`, `skills`, `commands`, `memory`, `worktree`, `review`, `pr`, `git`
- **Orchestration**: `workflows`, `goals`, `loops`, `teams`, `routines`, `handoff`, `teleport`, `apply`
- **Connectivity**: `serve`, `service`, `api`, `login`, `logout`, `whoami`, `tokens`, `peers`, `collab`, `remote`, `runners`, `runner`, `messages`, `channels`, `orchestrator`, `relay`, `share`, `github`, `acp`, `ide`
- **Extensibility**: `mcp`, `plugins`, `hooks`, `permissions`, `trust`, `sandbox`, `features`, `lsp`, `fmt`, `env`, `import`
- **Maintenance**: `doctor`, `debug`, `db`, `stats`, `telemetry`, `eval`, `upgrade`, `uninstall`, `completion`

This list is the single registry of top-level commands; no other spec SHALL introduce a top-level command absent from it. `import` SHALL import setups from other tools only (`compat-import`); session files and share URLs are imported with `cyber sessions import`. Commands and mode values whose owning phase has not shipped SHALL fail with exit 2 and an explicit unavailable-capability message. Parsing SHALL be strict: an unknown command or flag SHALL fail with exit code 2 and a "did you mean" suggestion when one is within edit distance 2.

#### Scenario: Unknown command suggestion
- **WHEN** the user runs `cyber sesions list`
- **THEN** the CLI prints `Error: unknown command "sesions". Did you mean "sessions"?` to stderr
- **AND** exits with code 2

#### Scenario: Help lists the tree
- **WHEN** the user runs `cyber --help`
- **THEN** every top-level command above is listed with a one-line description, grouped as Core, Orchestration, Connectivity, Extensibility and Maintenance

### Requirement: Global flags
(P0) Every command SHALL accept `--help`/`-h`, `--version`/`-V`, `--model`/`-m <provider/model[#variant]>`, `--agent <name>`, `--mode <default|accept-edits|plan|auto|dont-ask|bypass>`, `--cwd <path>`, `--add-dir <path>` (repeatable; `permissions-modes` → Additional working directories), `--profile <name>`, `-c`/`--config <key=value>` (repeatable; `configuration` → Command-line overrides), `--features <name,...>`, `--allow <rule>` and `--deny <rule>` (repeatable; Session ruleset entries `action[:resource]`), `--system-prompt <text|@file>` and `--append-system-prompt <text|@file>` (replace or extend the agent's `system` for the new Session), `--mcp-config <file>` (repeatable; adds MCP servers for the Session only, as `editor-integration` does for ACP clients), `--format <table|json|text|stream-json>` (the only output-format flag), `--print-logs`, `--log-level <trace|debug|info|warn|error>` and `--pure`. `--cwd` SHALL be the only working-directory flag. `--pure` SHALL disable external plugins, hooks, channels and MCP servers that are not managed by org policy. Flags SHALL take precedence over environment variables, and environment variables over config.

#### Scenario: Flag overrides config model
- **WHEN** `cyber.jsonc` sets `model` to `anthropic/claude-sonnet` and the user runs `cyber exec -m openai/gpt-6 "hi"`
- **THEN** the session uses `openai/gpt-6`

#### Scenario: Pure mode
- **WHEN** the user runs `cyber --pure`
- **THEN** no external plugin process is spawned, no user hooks run, and `cyber debug info` reports `external extensions disabled (--pure)`

#### Scenario: Inline allow rule for CI
- **WHEN** CI runs `cyber exec --allow "bash:npm test *" --deny "bash:git push *" "fix the tests"`
- **THEN** `npm test` runs without prompting, `git push` is denied, and both rules appear in the Session ruleset

### Requirement: Profiles
(P0) `--profile <name>` (or `CYBER_PROFILE`) SHALL select a named overlay from the `profiles` object in config, which SHALL be merged above all non-managed config layers. An unknown profile SHALL fail with exit code 2 and list the available profiles.

#### Scenario: Selecting a CI profile
- **WHEN** config defines `profiles.ci = { "mode": "dont-ask", "model": "openai/gpt-6-mini" }` and the user runs `cyber exec --profile ci "lint"`
- **THEN** the session uses mode `dont-ask` and model `openai/gpt-6-mini`

### Requirement: Default TUI command
(P0) `cyber [project]` SHALL ensure the background server is running (as `cyber service start` does), connect to it, and open the TUI on the Location for `project` (default: current directory). It SHALL accept `--continue`/`-c`, `--resume`/`-r [session]`, `--fork`, `--prompt <text>`, `--worktree [name]`, `--cloud <task>` and `--embedded`. `--embedded` SHALL run a private in-process server with no listener on a private database (`CYBER_DB`, or in-memory with `--ephemeral`) and SHALL never open the shared database for writing. Piped stdin SHALL be prepended to `--prompt`. `--fork` without `--continue` or `--resume` SHALL fail with exit code 2.

#### Scenario: Continue last session
- **WHEN** the user runs `cyber -c` in `/repo`
- **THEN** the TUI opens the most recently updated root Session whose Location is `/repo`

#### Scenario: Fork requires a source
- **WHEN** the user runs `cyber --fork`
- **THEN** the CLI fails with `Error: --fork requires --continue or --resume` and exit code 2

#### Scenario: Embedded TUI does not touch the shared database
- **WHEN** the user runs `cyber --embedded --ephemeral` while a background server is registered
- **THEN** the TUI runs against an in-memory private server and the registered server's database is not opened for writing

### Requirement: Exec command
(P0) `cyber exec [prompt..]` SHALL run one prompt non-interactively against a Session and exit when that Session's Drain becomes idle. It SHALL support the global `--format text|json|stream-json`, `--output-schema <file>`, `--session <id>`, `--continue`, `--goal <condition>`, `--attach <url>`, `--embedded`, `--file`/`-f` (repeatable), `--max-turns <n>`, `--max-cost <usd>`, `--timeout <duration>` and `--command <name>`. Detailed behavior is specified in the `exec-mode` capability.

#### Scenario: JSON output
- **WHEN** the user runs `cyber exec --format json "summarize README"`
- **THEN** stdout contains exactly one JSON object with `session_id`, `result`, `usage` and `cost_usd`

### Requirement: Review command
(P1) `cyber review [target]` SHALL run the local review (`vcs-integration` → Local review command) non-interactively. The target is empty (uncommitted changes), `<base>..<head>`, a commit, or `pr <number>`; it SHALL be validated with the review target grammar before any server contact. The command SHALL accept `--session`, `--continue`, `--embedded`, `--attach`, `--password`, `--timeout`, `--max-cost`, `--max-tokens`, the global `--format text|json|stream-json`, `--fail-on-deny`, `--ephemeral` and `--worktree`, with the meanings they have for `cyber exec`. It SHALL admit the `review` command through the same Session command path and owned-work rules as `cyber exec --command review` (`exec-mode` → Background work owned by the run). The `review` subcommand SHALL take precedence over a directory named `review`; `cyber ./review` opens the TUI there.

Exit codes SHALL be:
- 0: the review completed (with any number of findings), or the diff was empty.
- 1: the reviewer failed, returned an invalid result (`SchemaMismatch`), or its handback failed.
- 2: the target is invalid or could not be resolved (`invalid_target`, `invalid_ref`, `unresolved_ref`, `not_a_repository`, `gh_unavailable`, `pr_unresolved`, `pr_commits_missing`), or read policy leaves nothing reviewable (`all_withheld`: every changed file is withheld, or files were withheld and no remaining file has patch text), so changes nobody reviewed never pass as a clean tree.
- Permission denial, timeout and budget stops use the `cyber exec` exit codes. A review refused by a permission rule before admission (HTTP 403, such as an `external_directory` deny of the repository or a `bash` deny of `gh *` for `pr <number>`) SHALL exit 5 with `--fail-on-deny` and 1 otherwise, create no Job, and still print the result shape below with `findings: null`, `stop_reason: "error"` and the denial in `error`.
- A reviewer fork that fails or is cancelled before its Job exists (for example an `agent` rule denies `reviewer` at delegation admission) SHALL exit 1 with `reviewed: false` and the delegation's error.

`--format json` SHALL print one object, and `stream-json` SHALL end with a `result` line of the same shape: `target`, `target_kind` (`uncommitted|range|commit|pr`), `reviewed`, `findings` (validated and ordered as in `vcs-integration`), `limitations`, `counts` (`critical`, `high`, `medium`, `low`), `result` (the handback text), `session_id`, `parent_session_id`, `delegation_id`, `job_id`, `usage`, `cost_usd`, `duration_ms`, `stop_reason`, `error` and `exit_code`. `findings` SHALL be `null` when an overriding `review` skill or command ran or the review did not complete, and `[]` with `reviewed: false` when the diff was empty. `reviewed` SHALL be `true` only when validated findings were reported or an override completed. A target error SHALL add its `code`. Text output SHALL print the findings most severe first, then the limitations, or `No findings.`, `No changes to review` for an empty diff, and the handback text when an override ran.

#### Scenario: Review JSON output
- **WHEN** the user runs `cyber review main..HEAD --format json` and the reviewer reports one high finding
- **THEN** stdout is one JSON object with `target` `main..HEAD`, `target_kind` `range`, that finding in `findings`, `counts.high` 1 and `exit_code` 0

#### Scenario: Review refused by a permission rule
- **WHEN** `permissions.bash."gh *"` is `deny` and the user runs `cyber review --format json pr 3`
- **THEN** stdout is one review object with `target` `pr 3`, `findings: null` and the denial in `error`, the command exits 1 (5 with `--fail-on-deny`), and no Job is created

#### Scenario: Invalid target before any Session
- **WHEN** the user runs `cyber review a...b`
- **THEN** the command exits 2 with an `invalid_target` error and no Session is created

#### Scenario: Directory named review
- **WHEN** the Location contains a directory named `review` and the user runs `cyber ./review`
- **THEN** the TUI opens on that directory, while `cyber review` runs the review command

### Requirement: Server and service commands
(P0) `cyber serve` SHALL run the API server in the foreground on `--hostname` (default `127.0.0.1`) and `--port` (default `4747`, `0` = random). `cyber service start|stop|restart|status` SHALL manage one background server per user, registered in `<state>/server.json` (mode 0600) with `id`, `version`, `url` and `pid`. `service status` SHALL print `running <url>` or `stopped`. `cyber service password [value]` SHALL print or set the local server password.

#### Scenario: Reusing a healthy server
- **WHEN** a registered server answers `GET /api/v1/health` within 2 seconds with the same version and the user runs `cyber service start`
- **THEN** no new process is spawned and the existing URL is printed

#### Scenario: Version mismatch restarts the server
- **WHEN** the registered server reports version `1.2.0` and the CLI is `1.3.0`
- **THEN** `cyber service start` stops the old server and spawns a new detached `cyber serve --register`

### Requirement: Attach command
(P0) `cyber attach <url|session>` SHALL open the TUI against a running server or session. For a URL it SHALL accept `--password`/`-p` and `--username`/`-u`. For `<session>@<peer>` it SHALL attach through the configured peer (see `cross-session-messaging` → Peer attach). For a session reachable through the Relay it SHALL require a logged-in Cyber Account (see `remote-control`).

#### Scenario: Attach with password
- **WHEN** the user runs `cyber attach http://10.0.0.5:4747 -p s3cret`
- **THEN** the TUI connects with HTTP Basic credentials `cyber:s3cret`

#### Scenario: Attach to a peer Session
- **WHEN** the user runs `cyber attach migration@desk` and `desk` is a configured remote with an `attach` grant
- **THEN** the TUI opens that Session through the peer listener after `desk` approves `remote.attach`

### Requirement: Account commands
(P3) `cyber login [--no-browser] [--issuer <url>]`, `cyber logout [--all]` and `cyber whoami [--json]` SHALL manage the Cyber Account as specified in `cyber-account`. `whoami` SHALL print the subject, email, active org, entitlements and token expiry, or `not logged in` with exit code 0.

#### Scenario: Not logged in
- **WHEN** no account tokens are stored and the user runs `cyber whoami`
- **THEN** the CLI prints `not logged in (local features are fully available)` and exits 0

### Requirement: Resource command groups
(P0) Resource groups SHALL use consistent verbs: `list` (alias `ls`), `show <id>`, `delete <id>` (alias `rm`) and, where applicable, `create`, `stop`, `resume`. These groups SHALL exist with at least these subcommands:
- `sessions` (list, show, delete, tree, checkout, label, export, import, fork, rename, rewind, repair-context, share, unshare)
- `agents` (list, show, create), `models` (list, refresh), `providers` (list, login, logout, use), `skills` (list, show), `commands` (list), `memory` (list, show, edit, delete, path)
- `worktree` (list, remove, prune), `review`, `pr <number>`, `git` (commit-msg, pr-description), `web`
- `workflows` (run, list, runs, show, pause, resume, stop, logs), `goals` (set, list, show, pause, resume, clear), `loops` (add, list, show, pause, resume, run, rm, promote), `teams` (list, show, stop, resume), `routines` (create, list, show, edit, fire, pause, resume, rm, runs), `handoff`, `teleport <session>`, `apply <session>`
- `tokens` (create, list, revoke), `peers` (list, add, rm, test, discover, invite, revoke), `remote` (enable, disable, pair, devices, revoke, log), `runners` (list, use, logs), `runner` (start, drain), `messages` (list, send), `channels` (add, list, show, test, rotate-secret, log, disable, enable, rm), `orchestrator` (serve), `relay` (serve), `share` (serve), `github` (install), `acp`, `ide` (install)
- `mcp` (add, list, get, remove, auth, logout, debug, serve), `plugins` (install, list, info, update, remove, enable, disable, validate, marketplace, eval), `hooks` (list, trust, untrust, test), `permissions` (list, revoke, test, auto), `trust` (inspect, approve, revoke), `sandbox` (status, test, explain, run), `features` (list, enable, disable), `lsp` (status), `fmt` (status), `env` (create, list, edit, rm), `import` (claude, codex, opencode, auto, --detect)
- `db` (query, path, vacuum, backup, restore, encrypt, decrypt), `telemetry` (show), `eval` (run)

A singular alias of `sessions`, a standalone export command and a session-file import under `import` SHALL NOT exist. A Session argument of `cyber sessions`, `cyber -r` and `cyber exec --session` SHALL resolve, among the Location's Sessions, as an ID, then an exact title, then a unique ID prefix; a title or prefix that matches several Sessions SHALL exit 2 and list their IDs. `cyber sessions delete` SHALL require `--yes`.

#### Scenario: List output formats
- **WHEN** the user runs `cyber sessions list --format json`
- **THEN** stdout is a JSON array of objects with `id`, `title`, `directory`, `updated_at` and `status`

#### Scenario: Table paging
- **WHEN** `cyber sessions list` writes a table to a TTY without `--limit`
- **THEN** output is paged through `$PAGER` (default `less -RS`)

#### Scenario: Resume by title
- **WHEN** a Session of the Location is titled `Auth refactor` and the user runs `cyber exec --session "Auth refactor" "go on"`
- **THEN** the prompt is admitted to that Session

#### Scenario: Ambiguous title
- **WHEN** two Sessions of the Location are titled `same` and the user runs `cyber sessions show same`
- **THEN** the CLI exits 2 and lists both Session IDs

### Requirement: Raw API command
(P0) `cyber api <operationId | METHOD /path>` SHALL send one authenticated request to the background server (starting it if needed), accepting `-d`/`--data <json|@file>`, `-H`/`--header name:value` (up to 100) and `--param key=value`, and SHALL write the response body to stdout. Operation IDs SHALL be the OpenAPI operation IDs of the form `v1.<group>.<operation>`. A non-2xx response SHALL produce exit code 1.

#### Scenario: Call by operation ID
- **WHEN** the user runs `cyber api v1.session.list --param limit=5`
- **THEN** the CLI sends `GET /api/v1/sessions?limit=5` with the server credentials and prints the JSON body

### Requirement: Doctor and debug commands
(P0) `cyber doctor` SHALL check and report, as pass/warn/fail lines: config parse and validation, provider credentials, model catalog freshness, database integrity (`PRAGMA quick_check`), sandbox availability, ripgrep and git availability, LSP binaries, MCP connectivity, server health, account token validity, and instruction files. The `instructions` line SHALL list, by source path and in load order, the instruction files (`AGENTS.md`, `CLAUDE.md`, `CONTEXT.md`, `CYBER.md`, overrides and imports) that a new Session at the Location loads. It SHALL read `compat.claude` from the same per-Location config as the runtime, and SHALL warn about truncated or left-out files, files outside the project, not regular or refused by the `read` permission rules, and unresolved or refused imports. When the Location's config fails to load it SHALL list no project files, as the runtime loads none. It SHALL exit 1 when any check fails. `cyber debug` SHALL expose `config` (resolved, secrets redacted), `paths`, `info`, `agent <name>`, `tool <name> --params <json>`, `context` (rendered system context) and `events <session>`.

#### Scenario: Doctor detects missing sandbox dependency
- **WHEN** the user runs `cyber doctor` on Linux without `bwrap` installed
- **THEN** the sandbox line reports `fail: bubblewrap not found; install bubblewrap or set sandbox.enabled=false`
- **AND** the exit code is 1

#### Scenario: Debug config redaction
- **WHEN** config contains `providers.openai.api_key = "sk-abc"` and the user runs `cyber debug config`
- **THEN** the printed value is `"***"`

#### Scenario: Doctor lists CYBER.md
- **WHEN** the project contains `AGENTS.md` and a `CYBER.md` larger than `instructions.max_bytes`, and the user runs `cyber doctor`
- **THEN** the instructions line lists `AGENTS.md` and then `CYBER.md` by path, and warns that `CYBER.md` was truncated

### Requirement: Shell completion
(P0) `cyber completion <bash|zsh|fish|powershell>` SHALL print a completion script that covers commands, flags, agent names, model refs and session IDs. Dynamic values SHALL be resolved by calling `cyber __complete` with a 500 ms timeout.

#### Scenario: Completing a model ref
- **WHEN** the user types `cyber exec -m anth<TAB>` in zsh with completion installed
- **THEN** model refs beginning with `anthropic/` are offered

### Requirement: Process environment markers
(P0) Before dispatching, the CLI SHALL set `CYBER=1` and `CYBER_PID=<pid>` in its environment. Every child process started by tools, hooks, PTYs or plugins SHALL additionally receive `CYBER_SESSION_ID`, `CYBER_PROJECT_DIR` and, when applicable, `CYBER_AGENT` and `CYBER_WORKFLOW_RUN_ID`.

#### Scenario: Shell tool sees markers
- **WHEN** the bash tool runs `env | grep CYBER_SESSION_ID` in session `ses_01H`
- **THEN** the output contains `CYBER_SESSION_ID=ses_01H`

### Requirement: Logging destination
(P0) The CLI and server SHALL append structured JSON log lines to `<data>/log/cyber-<YYYY-MM-DD>.log`, keep 14 days of files, and write logs to stderr only with `--print-logs`. The minimum level SHALL default to `info` and follow `--log-level` or `CYBER_LOG_LEVEL`.

#### Scenario: Log rotation
- **WHEN** a log file is older than 14 days at startup
- **THEN** it is deleted

### Requirement: Error format and exit codes
(P0) Errors escaping a command SHALL print `Error: <message>` to stderr, followed by an optional `Hint: <text>` line, or as `{"error":{"code","message","hint"}}` when `--format json` is active. Exit codes SHALL be identical across all commands: 0 success, 1 runtime failure, 2 usage or configuration error, 3 goal judged impossible, 4 budget, turn or timeout limit exceeded, 5 permission denied or approval required in non-interactive mode, 6 Cyber Account or entitlement required, 130 interrupted by SIGINT.

#### Scenario: Missing provider credentials
- **WHEN** the user runs `cyber exec "hi"` and no provider is configured
- **THEN** stderr shows `Error: no model available` and `Hint: run "cyber providers login" or set OPENAI_API_KEY / ANTHROPIC_API_KEY`
- **AND** the exit code is 1

#### Scenario: Budget exit code
- **WHEN** `cyber exec --max-cost 0.50 "refactor"` reaches $0.50 of spend
- **THEN** the run stops and exits with code 4

### Requirement: Cloud and teleport entry points
(P3) `cyber --cloud "<task>"` SHALL create a new Session on the default Runner for the current repository and print its URL. `cyber teleport <session>` SHALL pull a remote Session into a local Location. `cyber handoff [--runner <id>]` SHALL move the current local Session to a Runner. Semantics are defined in `runners-cloud`.

#### Scenario: Cloud task without login
- **WHEN** the user runs `cyber --cloud "fix flaky test"` while not logged in
- **THEN** the CLI fails with `Error: this feature requires a Cyber Account` and `Hint: run "cyber login"`, exit code 6

### Requirement: Upgrade and uninstall commands
(P0) `cyber upgrade [version] [--method <m>]` and `cyber uninstall [--keep-config] [--keep-data] [--dry-run] [--force]` SHALL behave as specified in `installation-upgrade`.

#### Scenario: Dry-run uninstall
- **WHEN** the user runs `cyber uninstall --dry-run`
- **THEN** the paths that would be removed are listed with sizes and nothing is deleted

### Requirement: Stats command
(P0) `cyber stats [--days N] [--project <id>] [--models] [--tools] [--by origin] [--format json]` SHALL aggregate stored Sessions into totals of sessions, turns, tokens (input, output, reasoning, cache read, cache write), cost, tool usage, and per-model and per-workflow usage. (P1) Sessions, Turns, tokens, cost, streaks and the per-model table SHALL come from `GET /api/v1/usage/stats` of the shared server (`--days N` as `period=Nd`, 1 to 371; `--project <dir>` as `directory`, the project containing `<dir>`), so they match the TUI Stats tab. `--tools` SHALL count tool calls by name from the `tool_call` projection, for Sessions in scope that were active since the period's start. `--format json` SHALL print that response's `data` unchanged, plus `tools` when `--tools` is given. `--by origin` SHALL add one row per usage origin (`cross-session-messaging` → Usage attribution).

#### Scenario: Last seven days
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals include only Turns from the last 7 local days, with one row per model

#### Scenario: CLI and TUI agree
- **WHEN** the user runs `cyber stats --days 30 --format json` and opens the TUI Stats tab with `Last 30 days`
- **THEN** both show the same total tokens, Sessions and current streak

#### Scenario: Usage by origin
- **WHEN** the user runs `cyber stats --by origin` after prompting one Session and messaging another
- **THEN** separate rows for `user` and `session` show their turns, tokens and cost

### Requirement: Signal handling
(P0) On the first SIGINT during `exec` or the TUI, the CLI SHALL interrupt the active Drain, keeping all admitted input. A second SIGINT within 2 seconds SHALL exit with code 130. SIGTERM SHALL interrupt all Drains owned by the process and exit within 5 seconds.

#### Scenario: Double Ctrl+C
- **WHEN** the user presses Ctrl+C twice within 2 seconds during `cyber exec`
- **THEN** the run is interrupted and the process exits with code 130

### Requirement: Machine-readable version
(P0) `cyber --version` SHALL print `cyber <semver> (<channel>, <git-sha>, <target-triple>)`, and `cyber --version --format json` SHALL print the same fields as JSON. The HTTP User-Agent for outbound requests SHALL be `cyber/<version> (<os>; <arch>; <client>)`.

#### Scenario: Version output
- **WHEN** the user runs `cyber --version` on a stable build
- **THEN** the output matches `cyber 1.4.2 (stable, 3f2a9c1, aarch64-apple-darwin)`

### Requirement: Peers command
(P2) `cyber peers` SHALL manage direct peers: `list [--json]` prints remotes (name, URL, fingerprint prefix, state, latency) and grants (name, scopes, created, last used, expiry), flagging a grant with no remote of the same name; `add <name> <code>`, `rm <name>` (removes the remote and its stored token), `test <name>` (prints machine, version, scopes and latency, exit 1 with the error tag on failure), `discover`, `invite <name>` and `revoke <name>`. Tokens SHALL be printed only by `invite`, once.

#### Scenario: Grant without a way back
- **WHEN** `desk` has a grant `laptop` but no remote named `laptop`
- **THEN** `cyber peers list` on `desk` marks `laptop` `can receive, cannot reply` and prints the command to run on `laptop`

### Requirement: Session branch commands
(P1) `cyber sessions` SHALL provide:

- `tree <session>`: prints the user-message tree as ASCII with the active path marked, or as the server's tree nodes with `--format json`.
- `checkout <session> <message|label> [--code] [--exact]`: makes a branch active.
- `label <session> <name> [--at <message>] [--delete]`: sets or removes a label. Without `--at`, it labels the active tip.
- `export <session> [--all-branches]`: writes the Session and its messages as one JSON document, with the active path by default and every branch (each message with `parent_message_id`) with `--all-branches`.

A message argument SHALL accept an ID, a unique ID prefix or a label. A busy Session SHALL make `checkout` exit 1 with `SessionBusyError`, and an ambiguous prefix SHALL exit 2 and list the matching IDs.

#### Scenario: Checkout from the CLI
- **WHEN** the user runs `cyber sessions checkout "Auth refactor" fix-a --code`
- **THEN** the labeled branch becomes active, files are restored to its last snapshot, and the command prints the new tip and the restored file count

#### Scenario: Export every branch
- **WHEN** the user runs `cyber sessions export <session> --all-branches` on a Session with two branches
- **THEN** the document lists the messages of both branches in creation order with `branches: "all"`, and without the flag only the active path with `branches: "active"`
