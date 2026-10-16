## MODIFIED Requirements

### Requirement: Read-time instruction compatibility
(P0) Without any import, the system SHALL load these instruction files in the system context:
- `AGENTS.md` (global `~/.config/cyber/AGENTS.md`, plus walk-up project files)
- `CLAUDE.md` and `.claude/CLAUDE.md` (unless `compat.claude = false`, `CYBER_DISABLE_CLAUDE_COMPAT=1` or `CYBER_DISABLE_CLAUDE_MD=1`)
- `~/.claude/CLAUDE.md` as a global fallback only when `~/.config/cyber/AGENTS.md` does not exist

Within one directory, `AGENTS.md` SHALL take precedence and `CLAUDE.md` SHALL be included only when no `AGENTS.md` exists in that directory. `CYBER.md` is Cyber Code's native instruction file, not a compatibility file. No compatibility switch SHALL disable it, and it SHALL load after the compatibility files of its directory.

#### Scenario: Both files in one directory
- **WHEN** `/repo` contains both `AGENTS.md` and `CLAUDE.md`
- **THEN** only `/repo/AGENTS.md` is loaded for that directory

#### Scenario: Compatibility off keeps CYBER.md
- **WHEN** `compat.claude = false` and `/repo` contains `CLAUDE.md` and `CYBER.md`
- **THEN** only `/repo/CYBER.md` is loaded for that directory
