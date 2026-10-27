## MODIFIED Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register exactly these top-level commands, grouped as shown by `--help`:
- **Core**: the default TUI command `cyber [project]`, `exec`, `web`, `attach`, `sessions`, `models`, `providers`, `agents`, `skills`, `commands`, `memory`, `worktree`, `review`, `pr`, `git`, `computer`, `hex`
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


## ADDED Requirements

### Requirement: Hex editor command
(P1) `cyber hex <path> [--offset <decimal|0xhex>] [--read-only] [--session <id>]` SHALL open the same owner-backed hex workspace as the TUI without starting inference. It SHALL resolve the file against the selected Location/project and preserve Session/permission scope; outside-project targets SHALL require explicit appropriate working-directory selection rather than rebinding a reused Session. Without a terminal it SHALL fail with exit 2 and direct callers to the typed owner API instead of printing binary bytes. Existing global working-directory, model and Mode conventions SHALL remain compatible.

#### Scenario: Direct terminal entry
- **WHEN** an owner opens a project binary with `cyber hex fixtures/sample.bin --offset 0x20 --read-only`
- **THEN** the view starts at exact offset 32 without model inference, draft mutation or a write on exit
