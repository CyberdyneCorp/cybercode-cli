## MODIFIED Requirements

### Requirement: Saved and bundled workflows
(P2) The system SHALL discover saved Workflows from `.cyber/workflows/**/*.{ts,js}` (project), `~/.config/cyber/workflows/` (global) and plugins, and SHALL bundle `review` (multi-angle code review with verification), `audit` (codebase-wide bug or security sweep), `migrate` (file-by-file migration with build and test checks), `research` (cross-checked research with sources) `plan` (plans drafted from several independent angles, then merged), and `solve` (independent candidate implementations with host-run checks and an independent judge, as defined by `verified-workflows`). Project workflows SHALL override global and bundled ones with the same name.

Bundled Workflows SHALL be embedded in the `cyber` binary, carry the origin `bundled` with their name, and have the lowest precedence: global roots override them and project roots override both. A listing SHALL mark every entry's origin (`bundled`, `global` or `project`) and name the definitions it shadows. `workflows.bundled: false` or `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` SHALL hide the bundled Workflows from listings, the `workflow` tool and runs by name. `cyber workflows show <name>` SHALL print a library entry's meta, args schema, default budgets and origin (and with `--source` its source), and `cyber workflows eject <name> [--global] [--force]` SHALL copy a bundled Workflow's source to `.cyber/workflows/<name>.js` (or the global root), refusing to overwrite an existing file without `--force`. A run of a bundled Workflow SHALL record its origin and source digest, and a release that changes a bundled source SHALL be treated on resume like an edited saved module.

#### Scenario: Run a bundled workflow
- **WHEN** the user runs `cyber workflows run review --args '{"target":"main..HEAD"}'`
- **THEN** the bundled review workflow starts against the diff between the resolved commits of `main` and `HEAD`

#### Scenario: Bundled listed on a fresh install
- **WHEN** a user with no saved Workflows runs `cyber workflows list`
- **THEN** `review`, `audit`, `migrate`, `research`, `plan` and `solve` are listed with origin `bundled`

#### Scenario: Ejected copy overrides the bundled one
- **WHEN** the user runs `cyber workflows eject audit`, edits `.cyber/workflows/audit.js`, and runs `cyber workflows run audit`
- **THEN** the edited project module runs, and `cyber workflows list` shows `audit` with origin `project` shadowing `bundled:audit`

#### Scenario: Eject refuses to overwrite
- **WHEN** `.cyber/workflows/review.js` exists and the user runs `cyber workflows eject review`
- **THEN** the command exits 1 naming the file and leaves it unchanged

#### Scenario: First verified workflow slice
- **WHEN** a release ships the shared library foundation and `solve` before the five other planned modules
- **THEN** only implemented modules are listed as runnable, and `solve` is available with its real args schema, origin and budgets

#### Scenario: Start a verified solve
- **WHEN** the user runs `cyber workflows run solve --args` with a valid task and required checks
- **THEN** the server starts the verified candidate/check/judge protocol under the effective run ceilings
