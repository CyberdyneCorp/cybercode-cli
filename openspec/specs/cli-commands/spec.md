# cli-commands Specification

## Purpose
Defines the `cyber` command-line surface: the command tree, global flags, environment markers, error format and exit codes. Scripts, CI, documentation and users depend on stable names and behavior. The design merges OpenCode v1's yargs tree and v2's `service`/`api` commands, Codex's `exec` ergonomics, and Claude Code's `agents`, `remote-control` and `--cloud`/`--teleport` entry points. Each subsystem's internals are specified in its own capability; this spec only fixes the CLI contract.

## Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register these top-level commands: the default TUI command `cyber [project]`, `exec`, `serve`, `service`, `attach`, `login`, `logout`, `whoami`, `models`, `providers`, `agents`, `sessions`, `workflows`, `goals`, `loops`, `routines`, `remote`, `runners`, `messages`, `mcp`, `plugins`, `skills`, `hooks`, `sandbox`, `import`, `doctor`, `debug`, `db`, `stats`, `upgrade`, `uninstall`, `completion`, `trust`, `eval` and `api`. Commands and mode values whose owning phase has not shipped SHALL fail with exit 2 and an explicit unavailable-capability message. Parsing SHALL be strict: an unknown command or flag SHALL fail with exit code 2 and a "did you mean" suggestion when one is within edit distance 2.

#### Scenario: Unknown command suggestion
- **WHEN** the user runs `cyber sesions list`
- **THEN** the CLI prints `Error: unknown command "sesions". Did you mean "sessions"?` to stderr
- **AND** exits with code 2

#### Scenario: Help lists the tree
- **WHEN** the user runs `cyber --help`
- **THEN** every top-level command above is listed with a one-line description, grouped as Core, Orchestration, Connectivity, Extensibility and Maintenance

### Requirement: Global flags
(P0) Every command SHALL accept `--help`/`-h`, `--version`/`-V`, `--model`/`-m <provider/model[#variant]>`, `--agent <name>`, `--mode <default|accept-edits|plan|auto|dont-ask|bypass>`, `--cwd <path>`, `--profile <name>`, `--print-logs`, `--log-level <trace|debug|info|warn|error>` and `--pure`. `--pure` SHALL disable external plugins, hooks, channels and MCP servers that are not managed by org policy. Flags SHALL take precedence over environment variables, and environment variables over config.

#### Scenario: Flag overrides config model
- **WHEN** `cyber.jsonc` sets `model` to `anthropic/claude-sonnet` and the user runs `cyber exec -m openai/gpt-6 "hi"`
- **THEN** the session uses `openai/gpt-6`

#### Scenario: Pure mode
- **WHEN** the user runs `cyber --pure`
- **THEN** no external plugin process is spawned, no user hooks run, and `cyber debug info` reports `external extensions disabled (--pure)`

### Requirement: Profiles
(P0) `--profile <name>` (or `CYBER_PROFILE`) SHALL select a named overlay from the `profiles` object in config, which SHALL be merged above all non-managed config layers. An unknown profile SHALL fail with exit code 2 and list the available profiles.

#### Scenario: Selecting a CI profile
- **WHEN** config defines `profiles.ci = { "mode": "dont-ask", "model": "openai/gpt-6-mini" }` and the user runs `cyber exec --profile ci "lint"`
- **THEN** the session uses mode `dont-ask` and model `openai/gpt-6-mini`

### Requirement: Default TUI command
(P0) `cyber [project]` SHALL ensure the background server is running (as `cyber service start` does) and open the TUI on the Location for `project` (default: current directory). It SHALL accept `--continue`/`-c`, `--resume`/`-r [session]`, `--fork`, `--prompt <text>`, `--worktree [name]` and `--cloud <task>`. Piped stdin SHALL be prepended to `--prompt`. `--fork` without `--continue` or `--resume` SHALL fail with exit code 2.

#### Scenario: Continue last session
- **WHEN** the user runs `cyber -c` in `/repo`
- **THEN** the TUI opens the most recently updated root Session whose Location is `/repo`

#### Scenario: Fork requires a source
- **WHEN** the user runs `cyber --fork`
- **THEN** the CLI fails with `Error: --fork requires --continue or --resume` and exit code 2

### Requirement: Exec command
(P0) `cyber exec [prompt..]` SHALL run one prompt non-interactively against a Session and exit when that Session's Drain becomes idle. It SHALL support `--format text|json|stream-json`, `--output-schema <file>`, `--session <id>`, `--continue`, `--goal <condition>`, `--attach <url>`, `--file`/`-f` (repeatable), `--max-turns <n>`, `--max-cost <usd>` and `--command <name>`. Detailed behavior is specified in the `exec-mode` capability.

#### Scenario: JSON output
- **WHEN** the user runs `cyber exec --format json "summarize README"`
- **THEN** stdout contains exactly one JSON object with `session_id`, `result`, `usage` and `cost_usd`

### Requirement: Server and service commands
(P0) `cyber serve` SHALL run the API server in the foreground on `--hostname` (default `127.0.0.1`) and `--port` (default `4747`, `0` = random). `cyber service start|stop|restart|status` SHALL manage one background server per user, registered in `<state>/server.json` (mode 0600) with `id`, `version`, `url` and `pid`. `service status` SHALL print `running <url>` or `stopped`. `cyber service password [value]` SHALL print or set the local server password.

#### Scenario: Reusing a healthy server
- **WHEN** a registered server answers `GET /api/v1/health` within 2 seconds with the same version and the user runs `cyber service start`
- **THEN** no new process is spawned and the existing URL is printed

#### Scenario: Version mismatch restarts the server
- **WHEN** the registered server reports version `1.2.0` and the CLI is `1.3.0`
- **THEN** `cyber service start` stops the old server and spawns a new detached `cyber serve --register`

### Requirement: Attach command
(P0) `cyber attach <url|session>` SHALL open the TUI against a running server or session. For a URL it SHALL accept `--password`/`-p` and `--username`/`-u`. For a session reachable through the Relay it SHALL require a logged-in Cyber Account (see `remote-control`).

#### Scenario: Attach with password
- **WHEN** the user runs `cyber attach http://10.0.0.5:4747 -p s3cret`
- **THEN** the TUI connects with HTTP Basic credentials `cyber:s3cret`

### Requirement: Account commands
(P3) `cyber login [--no-browser] [--issuer <url>]`, `cyber logout [--all]` and `cyber whoami [--json]` SHALL manage the Cyber Account as specified in `cyber-account`. `whoami` SHALL print the subject, email, active org, entitlements and token expiry, or `not logged in` with exit code 0.

#### Scenario: Not logged in
- **WHEN** no account tokens are stored and the user runs `cyber whoami`
- **THEN** the CLI prints `not logged in (local features are fully available)` and exits 0

### Requirement: Resource command groups
(P0) Resource groups SHALL use consistent verbs: `list` (alias `ls`), `show <id>`, `delete <id>` (alias `rm`) and, where applicable, `create`, `stop`, `resume`. These groups SHALL exist: `sessions` (list, show, delete, export, import, fork, rename), `agents` (list, show, create), `models` (list, refresh), `providers` (list, login, logout), `workflows` (run, list, show, resume, stop, logs), `goals` (set, list, show, pause, resume, clear), `loops` (create, list, stop), `routines` (create, list, show, run, delete), `remote` (enable, disable, pair, devices, revoke), `runners` (list, register, remove), `messages` (list, send), `mcp` (add, list, auth, logout, serve), `plugins` (install, list, remove, update), `skills` (list, show), `hooks` (list, test) and `sandbox` (status, test).

#### Scenario: List output formats
- **WHEN** the user runs `cyber sessions list --format json`
- **THEN** stdout is a JSON array of objects with `id`, `title`, `directory`, `updated_at` and `status`

#### Scenario: Table paging
- **WHEN** `cyber sessions list` writes a table to a TTY without `--limit`
- **THEN** output is paged through `$PAGER` (default `less -RS`)

### Requirement: Raw API command
(P0) `cyber api <operationId | METHOD /path>` SHALL send one authenticated request to the background server (starting it if needed), accepting `-d`/`--data <json|@file>`, `-H`/`--header name:value` (up to 100) and `--param key=value`, and SHALL write the response body to stdout. A non-2xx response SHALL produce exit code 1.

#### Scenario: Call by operation ID
- **WHEN** the user runs `cyber api session.list --param limit=5`
- **THEN** the CLI sends `GET /api/v1/sessions?limit=5` with the server credentials and prints the JSON body

### Requirement: Doctor and debug commands
(P0) `cyber doctor` SHALL check and report, as pass/warn/fail lines: config parse and validation, provider credentials, model catalog freshness, database integrity (`PRAGMA quick_check`), sandbox availability, ripgrep and git availability, LSP binaries, MCP connectivity, server health and account token validity. It SHALL exit 1 when any check fails. `cyber debug` SHALL expose `config` (resolved, secrets redacted), `paths`, `info`, `agent <name>`, `tool <name> --params <json>`, `context` (rendered system context) and `events <session>`.

#### Scenario: Doctor detects missing sandbox dependency
- **WHEN** the user runs `cyber doctor` on Linux without `bwrap` installed
- **THEN** the sandbox line reports `fail: bubblewrap not found; install bubblewrap or set sandbox.enabled=false`
- **AND** the exit code is 1

#### Scenario: Debug config redaction
- **WHEN** config contains `providers.openai.api_key = "sk-abc"` and the user runs `cyber debug config`
- **THEN** the printed value is `"***"`

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
(P3) `cyber --cloud "<task>"` SHALL create a new Session on the default Runner for the current repository and print its URL. `cyber --teleport [session]` SHALL pull a remote Session into a local Location. `cyber handoff [--runner <id>]` SHALL move the current local Session to a Runner. Semantics are defined in `runners-cloud`.

#### Scenario: Cloud task without login
- **WHEN** the user runs `cyber --cloud "fix flaky test"` while not logged in
- **THEN** the CLI fails with `Error: this feature requires a Cyber Account` and `Hint: run "cyber login"`, exit code 6

### Requirement: Upgrade and uninstall commands
(P0) `cyber upgrade [version] [--method <m>]` and `cyber uninstall [--keep-config] [--keep-data] [--dry-run] [--force]` SHALL behave as specified in `installation-upgrade`.

#### Scenario: Dry-run uninstall
- **WHEN** the user runs `cyber uninstall --dry-run`
- **THEN** the paths that would be removed are listed with sizes and nothing is deleted

### Requirement: Stats command
(P0) `cyber stats [--days N] [--project <id>] [--models] [--tools] [--format json]` SHALL aggregate stored Sessions into totals of sessions, turns, tokens (input, output, reasoning, cache read, cache write), cost, tool usage, and per-model and per-workflow usage.

#### Scenario: Last seven days
- **WHEN** the user runs `cyber stats --days 7 --models`
- **THEN** totals include only Sessions updated in the last 7 days, with one row per model

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
