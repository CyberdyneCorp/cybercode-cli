## ADDED Requirements

### Requirement: Path-scoped rules and overrides
(P1) The system SHALL load rule files from `.cyber/rules/*.md` and, unless `compat.claude` is false, `.claude/rules/*.md` in each config directory. A rule file without frontmatter `paths` SHALL join `core/instructions` in the baseline. A rule file with `paths: [globs]` (relative to its project root) SHALL be attached through the nested-instruction mechanism the first time per Context Epoch that a matching file is read, edited or patched, labelled with its path. An `AGENTS.override.md` in a directory SHALL replace that directory's `AGENTS.md` and `CLAUDE.md` instead of being appended. Each instruction or rule file SHALL be truncated at `instructions.max_bytes` (default 32768) with a notice, and `cyber doctor` SHALL warn about truncated files.

#### Scenario: Migration rules only for migrations
- **WHEN** `.cyber/rules/migrations.md` has `paths: ["db/migrations/**"]` and the model edits `db/migrations/0042.sql`
- **THEN** the edit result includes the rule file in a `<system-reminder>` block, once for the epoch, and unrelated edits never load it

#### Scenario: Override file replaces
- **WHEN** `/repo/packages/api/` contains both `AGENTS.md` and `AGENTS.override.md`
- **THEN** only `AGENTS.override.md` is loaded for that directory
