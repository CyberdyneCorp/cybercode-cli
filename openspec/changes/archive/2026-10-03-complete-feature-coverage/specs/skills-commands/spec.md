## ADDED Requirements

### Requirement: Path-triggered and forked skills
(P1) `SKILL.md` frontmatter MAY declare `paths` (globs relative to the project root), `context` (`inline`, default, or `fork`) and `disallowed-tools`. When a file matching `paths` is first read, edited or patched in a Context Epoch, the skill's name and description SHALL be appended to the tool output in a `<system-reminder>` block suggesting `skill` be called. A skill with `context: fork` run as a user command or by the model SHALL execute in a forked background subagent (`agents-subagents` → Forked subagents) and return only its summary. `disallowed-tools` patterns SHALL be denied while the skill is active, taking precedence over `allowed-tools`.

#### Scenario: Skill suggested by path
- **WHEN** skill `migrations` declares `paths: ["db/migrations/**"]` and the model edits `db/migrations/0042.sql`
- **THEN** the edit result ends with a reminder naming the `migrations` skill, once for the epoch

#### Scenario: Forked skill
- **WHEN** the user runs `/deep-audit` and that skill has `context: fork`
- **THEN** a forked background subagent runs the skill and the parent receives its summary as a queued handback
