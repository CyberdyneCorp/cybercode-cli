## MODIFIED Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register exactly these top-level commands, grouped as shown by `--help`:
- **Core**: the default TUI command `cyber [project]`, `exec`, `web`, `attach`, `sessions`, `models`, `providers`, `agents`, `skills`, `commands`, `memory`, `worktree`, `review`, `pr`, `git`
- **Orchestration**: `workflows`, `goals`, `loops`, `teams`, `routines`, `handoff`, `teleport`, `apply`
- **Connectivity**: `serve`, `service`, `api`, `login`, `logout`, `whoami`, `tokens`, `peers`, `collab`, `remote`, `runners`, `runner`, `messages`, `channels`, `orchestrator`, `relay`, `share`, `github`, `acp`, `ide`
- **Extensibility**: `mcp`, `plugins`, `hooks`, `permissions`, `trust`, `sandbox`, `features`, `lsp`, `fmt`, `env`, `import`
- **Maintenance**: `doctor`, `debug`, `db`, `stats`, `telemetry`, `eval`, `upgrade`, `uninstall`, `completion`

`collab` is owned by the P3 `project-collaboration` capability. This list is the single registry of top-level commands; no other spec SHALL introduce a top-level command absent from it. `import` SHALL import setups from other tools only (`compat-import`); session files and share URLs are imported with `cyber sessions import`. Commands and mode values whose owning phase has not shipped SHALL fail with exit 2 and an explicit unavailable-capability message. Parsing SHALL be strict: an unknown command or flag SHALL fail with exit code 2 and a "did you mean" suggestion when one is within edit distance 2.

#### Scenario: Unknown command suggestion
- **WHEN** the user runs `cyber sesions list`
- **THEN** the CLI prints `Error: unknown command "sesions". Did you mean "sessions"?` to stderr
- **AND** exits with code 2

#### Scenario: Help lists the tree
- **WHEN** the user runs `cyber --help`
- **THEN** every top-level command above is listed with a one-line description, grouped as Core, Orchestration, Connectivity, Extensibility and Maintenance


## ADDED Requirements

### Requirement: Project collaboration commands
(P3) `cyber collab` SHALL expose `serve`, `project create|list|show`, `invite`, `invitations list|revoke`, `join`, `bind`, `members list|set-role|remove`, `publish`, `unpublish`, `messages list|send`, `tasks list|create|claim|update|reconcile`, and `leave`, with explicit project IDs, typed JSON output and the authorization/consent rules of `project-collaboration`. `serve` SHALL accept the trusted issuer, API audience, bootstrap subject, listen address (default `127.0.0.1:4749`) and operator TLS configuration. Local commands SHALL use the owning local server rather than a second untracked runtime. The feature SHALL remain explicitly unavailable until enabled and implemented; parsing a reserved command SHALL NOT create state or pretend it succeeded. Human message/task operations SHALL require no LLM request.

#### Scenario: Human posts without an agent
- **WHEN** an authorized member runs `cyber collab messages send --project <id>` with text
- **THEN** the hub records that authenticated human author without starting a Session or invoking a model

#### Scenario: Disabled feature
- **WHEN** a user runs a collaboration mutation while `features.collaboration` is off
- **THEN** the command reports the disabled capability and creates no membership, publication or message
