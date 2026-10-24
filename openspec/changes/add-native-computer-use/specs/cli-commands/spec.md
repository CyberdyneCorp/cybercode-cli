## MODIFIED Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register exactly these top-level commands, grouped as shown by `--help`:
- **Core**: the default TUI command `cyber [project]`, `exec`, `web`, `attach`, `sessions`, `models`, `providers`, `agents`, `skills`, `commands`, `memory`, `worktree`, `review`, `pr`, `git`, `computer`
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

### Requirement: Computer use owner commands
(P2) `cyber computer status|setup|allow|stop` SHALL expose native capability/OS-consent health, deliberate OS setup, owner-armed Session/target grants and immediate revocation. It SHALL clearly distinguish disabled, unsupported, missing OS consent, unarmed, active and stopped states. Models and hub agents SHALL NOT call the owner arming path as a substitute for human consent.

#### Scenario: Inspect capability without capturing
- **WHEN** the owner runs `cyber computer status`
- **THEN** it reports platform and OS/grant readiness without recording the screen, posting input or prompting for OS consent
