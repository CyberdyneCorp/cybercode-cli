## MODIFIED Requirements

### Requirement: Launch flags
(P0) The system SHALL accept these launch flags:
- `--continue`/`-c`: resume the most recent root Session of the Location.
- `--resume [id|name]`/`-r`: resume a Session by ID or name, or open the picker when no value is given.
- `--fork`: copy the selected Session into a new one.
- `--model provider/model[#variant]`, `--agent <name>`, `--mode <mode>`.
- `--worktree [name]`: start in a managed worktree.
- `--prompt <text>`: submit an initial prompt.
- `--dangerously-skip-permissions`: with `--embedded`, start in `bypass` with the override of `--dangerously-bypass-permissions`.

Piped stdin SHALL be prepended to `--prompt`. Invalid combinations SHALL fail before the UI renders, with exit code 2, including `--dangerously-skip-permissions` without `--embedded` or with a `--mode` other than `bypass`.

#### Scenario: Fork requires a source session
- **WHEN** a user runs `cyber --fork` without `--continue` or `--resume`
- **THEN** the CLI prints `--fork requires --continue or --resume` and exits with code 2 without rendering the UI

#### Scenario: Piped stdin becomes the prompt
- **WHEN** a user runs `git diff | cyber --prompt "review this"`
- **THEN** the first submitted message contains the diff text followed by `review this`

#### Scenario: Resume by name
- **WHEN** a user runs `cyber -r auth-refactor` and exactly one Session in the Location is named `auth-refactor`
- **THEN** the TUI opens that Session with its history rendered and its stored agent, model and mode restored

#### Scenario: Skip flag needs an embedded server
- **WHEN** a user runs `cyber --dangerously-skip-permissions` without `--embedded`
- **THEN** the CLI prints `--dangerously-skip-permissions needs --embedded` and exits with code 2 without rendering the UI
