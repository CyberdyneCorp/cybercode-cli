## MODIFIED Requirements

### Requirement: Model roles
(P0) `model_roles` SHALL map the roles `default`, `small`, `advisor`, `evaluator`, `compaction` and `title` to model refs `provider/model[#variant]`. `model` SHALL be shorthand for `model_roles.default`, and `small_model` for `model_roles.small`. When a role is unset, `title` and `compaction` SHALL fall back to `small`, then `default`; `small` SHALL fall back to `default`; `evaluator` SHALL fall back to `small`, then `default`; `advisor` SHALL be disabled.

#### Scenario: Role fallback
- **WHEN** config sets only `model = "anthropic/claude-sonnet"` and `small_model = "openai/gpt-6-mini"`
- **THEN** session titles and goal evaluation use `openai/gpt-6-mini`, and the advisor is off

### Requirement: Layering order
(P0) The system SHALL merge layers from lowest to highest priority as follows:
1. built-in defaults
2. global `cyber.json` then `cyber.jsonc`
3. project documents from the farthest ancestor to the nearest (`.json` before `.jsonc` in the same directory)
4. `.cyber/` directories from farthest to nearest
5. `CYBER_CONFIG` file
6. `CYBER_CONFIG_CONTENT` inline JSON
7. the selected profile
8. CLI flags
9. org-managed policy layers, which are always last (see `org-policy`)

Project layers SHALL pass workspace-trust before activating security-sensitive values or substitutions. Objects SHALL deep-merge. Scalars and arrays SHALL be replaced, except `instructions`, `plugins` and `skills`, which SHALL be concatenated and de-duplicated.

#### Scenario: Nearest wins for scalars
- **WHEN** the global config sets `mode = "default"` and `/repo/cyber.jsonc` sets `mode = "accept-edits"`
- **THEN** sessions in `/repo` start in mode `accept-edits`

#### Scenario: Instructions concatenate
- **WHEN** the global config sets `instructions = ["~/rules.md"]` and the project sets `instructions = ["docs/style.md"]`
- **THEN** both files are loaded, global first
