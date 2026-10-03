## MODIFIED Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register exactly these top-level commands, grouped as shown by `--help`:
- **Core**: the default TUI command `cyber [project]`, `exec`, `attach`, `sessions`, `models`, `providers`, `agents`, `skills`, `commands`, `memory`, `worktree`, `review`, `pr`, `git`
- **Orchestration**: `workflows`, `goals`, `loops`, `teams`, `routines`, `handoff`, `teleport`
- **Connectivity**: `serve`, `service`, `api`, `login`, `logout`, `whoami`, `remote`, `runners`, `runner`, `messages`, `channels`, `orchestrator`, `relay`, `share`, `github`, `acp`, `ide`
- **Extensibility**: `mcp`, `plugins`, `hooks`, `permissions`, `trust`, `sandbox`, `lsp`, `fmt`, `env`, `import`
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
(P0) Every command SHALL accept `--help`/`-h`, `--version`/`-V`, `--model`/`-m <provider/model[#variant]>`, `--agent <name>`, `--mode <default|accept-edits|plan|auto|dont-ask|bypass>`, `--cwd <path>`, `--profile <name>`, `--format <table|json|text|stream-json>` (the only output-format flag; `table` on a TTY and `json` otherwise for listing commands, `text` for `exec`), `--print-logs`, `--log-level <trace|debug|info|warn|error>` and `--pure`. `--cwd` SHALL be the only working-directory flag. `--pure` SHALL disable external plugins, hooks, channels and MCP servers that are not managed by org policy. Flags SHALL take precedence over environment variables, and environment variables over config.

#### Scenario: Flag overrides config model
- **WHEN** `cyber.jsonc` sets `model` to `anthropic/claude-sonnet` and the user runs `cyber exec -m openai/gpt-6 "hi"`
- **THEN** the session uses `openai/gpt-6`

#### Scenario: Pure mode
- **WHEN** the user runs `cyber --pure`
- **THEN** no external plugin process is spawned, no user hooks run, and `cyber debug info` reports `external extensions disabled (--pure)`

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

### Requirement: Resource command groups
(P0) Resource groups SHALL use consistent verbs: `list` (alias `ls`), `show <id>`, `delete <id>` (alias `rm`) and, where applicable, `create`, `stop`, `resume`. These groups SHALL exist with at least these subcommands:
- `sessions` (list, show, delete, export, import, fork, rename, rewind, repair-context, share, unshare)
- `agents` (list, show, create), `models` (list, refresh), `providers` (list, login, logout, use), `skills` (list, show), `commands` (list), `memory` (list, show, edit, delete, path)
- `worktree` (list, remove, prune), `review`, `pr <number>`, `git` (commit-msg, pr-description)
- `workflows` (run, list, runs, show, pause, resume, stop, logs), `goals` (set, list, show, pause, resume, clear), `loops` (add, list, show, pause, resume, run, rm, promote), `teams` (list, show, stop, resume), `routines` (create, list, show, edit, fire, pause, resume, rm, runs), `handoff`, `teleport <session>`
- `remote` (enable, disable, pair, devices, revoke, log), `runners` (list, use, logs), `runner` (start, drain), `messages` (list, send), `channels` (add, list, show, test, rotate-secret, log, disable, enable, rm), `orchestrator` (serve), `relay` (serve), `share` (serve), `github` (install), `acp`, `ide` (install)
- `mcp` (add, list, get, remove, auth, logout, debug, serve), `plugins` (install, list, info, update, remove, enable, disable, validate, marketplace, eval), `hooks` (list, trust, untrust, test), `permissions` (list, revoke), `trust` (inspect, approve, revoke), `sandbox` (status, test, explain, run), `lsp` (status), `fmt` (status), `env` (create, list, edit, rm), `import` (claude, codex, opencode, auto, --detect)
- `db` (query, path, vacuum, backup, restore, encrypt, decrypt), `telemetry` (show), `eval` (run)

Singular forms (`cyber session ...`) SHALL NOT exist; `cyber export` and a session-file `cyber import` SHALL NOT exist.

#### Scenario: List output formats
- **WHEN** the user runs `cyber sessions list --format json`
- **THEN** stdout is a JSON array of objects with `id`, `title`, `directory`, `updated_at` and `status`

#### Scenario: Table paging
- **WHEN** `cyber sessions list` writes a table to a TTY without `--limit`
- **THEN** output is paged through `$PAGER` (default `less -RS`)

### Requirement: Raw API command
(P0) `cyber api <operationId | METHOD /path>` SHALL send one authenticated request to the background server (starting it if needed), accepting `-d`/`--data <json|@file>`, `-H`/`--header name:value` (up to 100) and `--param key=value`, and SHALL write the response body to stdout. Operation IDs SHALL be the OpenAPI operation IDs of the form `v1.<group>.<operation>`. A non-2xx response SHALL produce exit code 1.

#### Scenario: Call by operation ID
- **WHEN** the user runs `cyber api v1.session.list --param limit=5`
- **THEN** the CLI sends `GET /api/v1/sessions?limit=5` with the server credentials and prints the JSON body

### Requirement: Cloud and teleport entry points
(P3) `cyber --cloud "<task>"` SHALL create a new Session on the default Runner for the current repository and print its URL. `cyber teleport <session>` SHALL pull a remote Session into a local Location. `cyber handoff [--runner <id>]` SHALL move the current local Session to a Runner. Semantics are defined in `runners-cloud`.

#### Scenario: Cloud task without login
- **WHEN** the user runs `cyber --cloud "fix flaky test"` while not logged in
- **THEN** the CLI fails with `Error: this feature requires a Cyber Account` and `Hint: run "cyber login"`, exit code 6
