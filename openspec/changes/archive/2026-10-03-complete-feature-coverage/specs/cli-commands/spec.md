## MODIFIED Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register exactly these top-level commands, grouped as shown by `--help`:
- **Core**: the default TUI command `cyber [project]`, `exec`, `web`, `attach`, `sessions`, `models`, `providers`, `agents`, `skills`, `commands`, `memory`, `worktree`, `review`, `pr`, `git`
- **Orchestration**: `workflows`, `goals`, `loops`, `teams`, `routines`, `handoff`, `teleport`, `apply`
- **Connectivity**: `serve`, `service`, `api`, `login`, `logout`, `whoami`, `tokens`, `peers`, `remote`, `runners`, `runner`, `messages`, `channels`, `orchestrator`, `relay`, `share`, `github`, `acp`, `ide`
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

### Requirement: Resource command groups
(P0) Resource groups SHALL use consistent verbs: `list` (alias `ls`), `show <id>`, `delete <id>` (alias `rm`) and, where applicable, `create`, `stop`, `resume`. These groups SHALL exist with at least these subcommands:
- `sessions` (list, show, delete, export, import, fork, rename, rewind, repair-context, share, unshare)
- `agents` (list, show, create), `models` (list, refresh), `providers` (list, login, logout, use), `skills` (list, show), `commands` (list), `memory` (list, show, edit, delete, path)
- `worktree` (list, remove, prune), `review`, `pr <number>`, `git` (commit-msg, pr-description), `web`
- `workflows` (run, list, runs, show, pause, resume, stop, logs), `goals` (set, list, show, pause, resume, clear), `loops` (add, list, show, pause, resume, run, rm, promote), `teams` (list, show, stop, resume), `routines` (create, list, show, edit, fire, pause, resume, rm, runs), `handoff`, `teleport <session>`, `apply <session>`
- `tokens` (create, list, revoke), `peers` (list, add, rm, test, discover), `remote` (enable, disable, pair, devices, revoke, log), `runners` (list, use, logs), `runner` (start, drain), `messages` (list, send), `channels` (add, list, show, test, rotate-secret, log, disable, enable, rm), `orchestrator` (serve), `relay` (serve), `share` (serve), `github` (install), `acp`, `ide` (install)
- `mcp` (add, list, get, remove, auth, logout, debug, serve), `plugins` (install, list, info, update, remove, enable, disable, validate, marketplace, eval), `hooks` (list, trust, untrust, test), `permissions` (list, revoke, test, auto), `trust` (inspect, approve, revoke), `sandbox` (status, test, explain, run), `features` (list, enable, disable), `lsp` (status), `fmt` (status), `env` (create, list, edit, rm), `import` (claude, codex, opencode, auto, --detect)
- `db` (query, path, vacuum, backup, restore, encrypt, decrypt), `telemetry` (show), `eval` (run)

A singular alias of `sessions`, a standalone export command and a session-file import under `import` SHALL NOT exist.

#### Scenario: List output formats
- **WHEN** the user runs `cyber sessions list --format json`
- **THEN** stdout is a JSON array of objects with `id`, `title`, `directory`, `updated_at` and `status`

#### Scenario: Table paging
- **WHEN** `cyber sessions list` writes a table to a TTY without `--limit`
- **THEN** output is paged through `$PAGER` (default `less -RS`)
