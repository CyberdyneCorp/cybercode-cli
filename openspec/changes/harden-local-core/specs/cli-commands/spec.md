## MODIFIED Requirements

### Requirement: Command tree
(P0) The CLI SHALL be invoked as `cyber` and SHALL register these top-level commands: the default TUI command `cyber [project]`, `exec`, `serve`, `service`, `attach`, `login`, `logout`, `whoami`, `models`, `providers`, `agents`, `sessions`, `workflows`, `goals`, `loops`, `routines`, `remote`, `runners`, `messages`, `mcp`, `plugins`, `skills`, `hooks`, `sandbox`, `import`, `doctor`, `debug`, `db`, `stats`, `upgrade`, `uninstall`, `completion`, `trust`, `eval` and `api`. Commands and mode values whose owning phase has not shipped SHALL fail with exit 2 and an explicit unavailable-capability message. Parsing SHALL be strict: an unknown command or flag SHALL fail with exit code 2 and a "did you mean" suggestion when one is within edit distance 2.

#### Scenario: Unknown command suggestion
- **WHEN** the user runs `cyber sesions list`
- **THEN** the CLI prints `Error: unknown command "sesions". Did you mean "sessions"?` to stderr
- **AND** exits with code 2

#### Scenario: Help lists the tree
- **WHEN** the user runs `cyber --help`
- **THEN** every top-level command above is listed with a one-line description, grouped as Core, Orchestration, Connectivity, Extensibility and Maintenance
