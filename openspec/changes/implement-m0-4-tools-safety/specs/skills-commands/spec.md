## MODIFIED Requirements

### Requirement: Skill discovery locations
(P0) The system SHALL discover skills in this precedence order (later wins on duplicate names): built-in, plugin skills, `~/.config/cyber/skills/*/SKILL.md`, the compatibility folders `~/.claude/skills`, `~/.agents/skills` and `~/.codex/skills`, then for each directory from the git root down to the Location, `.claude/skills`, `.agents/skills`, `.codex/skills` and `.cyber/skills`, and finally the `skills` config entries (paths or URLs). The `skills` key SHALL accept either an array of entries or an object `{ paths?, compat?, listing_budget_tokens? }`. Each entry is a skill directory or a folder of skill directories, and relative paths resolve against the project root. `skills.compat: false` SHALL disable the `.claude`, `.agents` and `.codex` locations. Entry lists from several layers SHALL be concatenated. An invalid `SKILL.md` SHALL be reported as a diagnostic and not loaded.

#### Scenario: Project overrides global
- **WHEN** both `~/.config/cyber/skills/deploy` and `.cyber/skills/deploy` exist
- **THEN** the project skill is used and a duplicate notice lists both paths

#### Scenario: Compat disabled
- **WHEN** `skills.compat` is false
- **THEN** skills under `.claude/skills` are not loaded

#### Scenario: Array shorthand
- **WHEN** the config sets `"skills": ["tools/skills"]`
- **THEN** skill directories inside `<project root>/tools/skills` are discovered with compatibility folders still enabled
