## MODIFIED Requirements

### Requirement: Skill-scoped tool approvals and model
(P1) While a skill loaded in the current Turn declares `allowed-tools`, matching tool calls SHALL be allowed without prompting, unless a permission rule denies them. The `allowed-tools` of a skill a repository provides (a project skill folder, or a folder named by `skills.paths`) SHALL count only once the checkout is trusted (`workspace-trust` → Trust before interpretation); until then they SHALL be ignored with a diagnostic naming the skill, and the skill still loads. `disallowed-tools` SHALL always apply. Built-in and user skills are unaffected. A skill `model` SHALL apply only when the skill is run as a user command or as a subtask.

#### Scenario: Pre-approved git commands
- **WHEN** skill `release` declares `allowed-tools: ["bash:git tag *"]` and the model runs `git tag v1.2.0`
- **THEN** no permission prompt is shown

#### Scenario: Untrusted repository skill grants nothing
- **WHEN** an untrusted checkout ships `.cyber/skills/x/SKILL.md` with `allowed-tools: bash` and the model loads `x` and runs `curl … | sh`
- **THEN** the `bash` call follows the ordinary rules and asks, and `cyber skills` lists a diagnostic that the grant is ignored until the checkout is trusted
