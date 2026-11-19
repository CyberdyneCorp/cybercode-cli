## MODIFIED Requirements

### Requirement: Model roles
(P0) `model_roles` SHALL map the roles `default`, `small`, `advisor`, `evaluator`, `compaction`, `title` and `plan` to model refs `provider/model[#variant]`. `model` SHALL be shorthand for `model_roles.default`, and `small_model` for `model_roles.small`. When a role is unset, `title` and `compaction` SHALL fall back to `small`, then `default`; `small` SHALL fall back to `default`; `evaluator` SHALL fall back to `small`, then `default`; `advisor` and `plan` SHALL be disabled.

#### Scenario: Role fallback
- **WHEN** config sets only `model = "anthropic/claude-sonnet"` and `small_model = "openai/gpt-6-mini"`
- **THEN** session titles and goal evaluation use `openai/gpt-6-mini`, and the advisor and the planner are off

#### Scenario: Planner configured
- **WHEN** the global config sets `model_roles.plan = "anthropic/claude-opus"`
- **THEN** plan-mode Turns of top-level Sessions run on `anthropic/claude-opus`
