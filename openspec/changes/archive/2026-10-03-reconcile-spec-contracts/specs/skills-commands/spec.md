## MODIFIED Requirements

### Requirement: Skills as user commands
(P0) Each skill with `user-invocable` not false SHALL be invocable as `/<name> [args]`. The skill body SHALL be treated as a command template: `$ARGUMENTS` and `$1..$N` SHALL be substituted exactly as for custom commands, and when the body has no placeholders and arguments are non-empty, the arguments SHALL be appended after a blank line as `ARGUMENTS: <args>`. A skill with `disable-model-invocation: true` SHALL be omitted from `<available_skills>` and loadable only by the user.

#### Scenario: User runs a skill
- **WHEN** the user types `/release minor` and the skill body contains `Cut a $1 release`
- **THEN** the prompt submitted is the body with `Cut a minor release`

#### Scenario: User runs a skill without placeholders
- **WHEN** the user types `/release minor` and the body has no placeholders
- **THEN** the body is submitted followed by a blank line and `ARGUMENTS: minor`

### Requirement: Bundled skills
(P1) The system SHALL bundle skills `review` (single-agent code review of a diff producing the structured findings defined by `vcs-integration`), `batch` (split a large change into 5-30 worktree-isolated subagents), `simplify` (review changed code for reuse and simplification), `security-review` (review pending changes for vulnerabilities) and `customize-cyber` (how to edit Cyber Code config, agents, skills, hooks and MCP). Each SHALL be replaceable by a discovered skill of the same name.

#### Scenario: Override bundled skill
- **WHEN** a project defines `.cyber/skills/simplify/SKILL.md`
- **THEN** the project version replaces the bundled one
