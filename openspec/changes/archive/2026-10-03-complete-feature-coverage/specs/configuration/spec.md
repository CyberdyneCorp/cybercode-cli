## MODIFIED Requirements

### Requirement: Top-level keys
(P0) The schema SHALL accept the top-level keys `$schema`, `model`, `small_model`, `model_roles`, `fallback_models`, `providers`, `agents`, `default_agent`, `mode`, `permissions`, `sandbox`, `hooks`, `plugins`, `mcp`, `skills`, `commands`, `instructions`, `references`, `memory`, `compaction`, `snapshots`, `worktrees`, `lsp`, `formatters`, `tool_output`, `workflows`, `goals`, `loops`, `messaging`, `peers`, `remote`, `runners`, `channels`, `account`, `share`, `telemetry`, `budgets`, `policy`, `profiles`, `default_profile`, `shell`, `attachments`, `background`, `tools`, `git`, `ide`, `autofix`, `network`, `storage`, `compat`, `services`, `features` and `experimental` (deprecated alias of `features`). Collection keys SHALL use plural names, and entries SHALL use `disabled: true` to turn off an inherited entry.

#### Scenario: Disabling an inherited MCP server
- **WHEN** the global config defines `mcp.github` and the project config sets `mcp.github.disabled = true`
- **THEN** the `github` MCP server is not started for that project

## ADDED Requirements

### Requirement: Feature flags registry
(P0) The binary SHALL embed a registry of feature names, each with a maturity label `experimental`, `beta` or `stable` and a default (`experimental` off, others on). `features.<name>: true|false` in config, `--features a,b,-c` and `CYBER_FEATURES` SHALL toggle them; an unknown name SHALL fail with `ConfigInvalidError` naming it. `cyber features list [--json]` SHALL print every feature with maturity, default, effective value and source; `cyber features enable|disable <name> [--scope user|project]` SHALL edit config. Org policy MAY lock features (`features.lock`). The `experimental` key SHALL be accepted as an alias with a deprecation warning. Specs introducing a gated feature SHALL name its registry entry (for example `teams`).

#### Scenario: Unknown feature rejected
- **WHEN** config sets `features.telepathy: true`
- **THEN** loading fails with `ConfigInvalidError: unknown feature "telepathy"`

#### Scenario: List features
- **WHEN** the user runs `cyber features list`
- **THEN** `teams experimental off (default)` and the other entries are printed with their sources

### Requirement: Command-line overrides
(P0) `-c`/`--config <key=value>` SHALL set one config value for the process, where `key` is a dotted path and `value` is parsed as JSON5 when it parses and as a string otherwise. Overrides SHALL merge as a layer above the selected profile and below other CLI flags, SHALL be validated against the schema, and SHALL NOT be able to set `policy` or `profiles`. `cyber debug config` SHALL show the source `cli:-c` for overridden values.

#### Scenario: One-off override
- **WHEN** the user runs `cyber exec -c compaction.auto=false -c 'sandbox.allowed_domains=["example.org"]' "..."`
- **THEN** the Session runs without automatic compaction, with `example.org` allowed, and nothing is written to any config file

### Requirement: References
(P1) `references` SHALL be a list of `{ alias, path | git: { url, ref? }, description? }`. Local paths SHALL be read in place; git references SHALL be cloned shallowly into `<cache>/references/<alias>-<hash>` and refreshed at most every 24 hours (`cyber references refresh` is part of `cyber debug`). The system SHALL list references for the agent in the `core/references` Context Source as `<available_references>` entries with alias and description, allow `@<alias>/<path>` mentions and autocomplete, and add each reference root to the `external_directory` read allow list. Project-defined git references SHALL be inactive until approved by workspace-trust, because they fetch from the network.

#### Scenario: Reference a sibling repository
- **WHEN** config defines `references: [{ alias: "api-spec", git: { url: "git@github.com:acme/api-spec.git" }, description: "OpenAPI documents" }]`
- **THEN** the model sees `api-spec` in `<available_references>` and `read` on `@api-spec/openapi.yaml` succeeds without an external-directory prompt
