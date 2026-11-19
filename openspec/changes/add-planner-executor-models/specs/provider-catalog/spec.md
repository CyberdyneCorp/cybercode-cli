## MODIFIED Requirements

### Requirement: Model roles
(P0) The system SHALL resolve the roles `default`, `small`, `advisor`, `evaluator`, `compaction`, `title` and `plan` from `model_roles` config, using the canonical configuration fallback: `default` uses `model`; `small` uses `small_model` then `default`; `title` and `compaction` use `small` then `default`; `evaluator` uses `small` then `default`; an unset `advisor` is disabled; an unset `plan` is disabled (`provider-catalog` → Planner and executor models). Workflows MAY define named tiers under `workflows.tiers` (for example `fast`, `strong`) that resolve to model references.

#### Scenario: Title uses the small role
- **WHEN** `model_roles.title` is unset and `small_model` is `openai/gpt-6-mini`
- **THEN** session titles are generated with `openai/gpt-6-mini`

#### Scenario: Plan role has no fallback
- **WHEN** `model_roles.plan` is unset and `model` is `openai/gpt-6`
- **THEN** the `plan` role resolves to nothing and plan-mode Turns use `openai/gpt-6`

## ADDED Requirements

### Requirement: Planner and executor models
(P1) When `model_roles.plan` is set, every Turn of a Session without a parent whose Mode is `plan` SHALL run on the plan model (the planner), and every other Turn SHALL run on the Session's own model (the executor), with no other configuration. The selection SHALL be made at each Safe Boundary in this order: a command's own model, then an active fallback, then the planner, then the Session's model selection; a change of the resolved model SHALL be recorded as `session.model.switched.1` with `automatic: true`, which leaves the Session's own selection unchanged. Leaving `plan` (an approved `plan_exit`, a Mode switch from a client) SHALL therefore return the next Turn to the executor, and entering `plan` SHALL move the next Turn to the planner. An explicit user model switch while the Session's Mode is `plan` SHALL take effect at the next Turn, SHALL become the Session's model as any explicit switch does, and SHALL suspend the planner until the Session's Mode changes again. Sessions with a parent (subagents, team members, workflow agents) SHALL keep their own model resolution. `session.step.started.1` SHALL carry `model_role: "plan"` for a step run on the planner and omit it otherwise, and the message entry SHALL expose the same field. The Session view SHALL report `model_role: "plan"` while the planner is the Session's active model, and the TUI SHALL show `planner: <model>` in its status line while it is reported. With `model_roles.plan` unset, model selection, events and views SHALL be unchanged.

#### Scenario: Plan on one model, build on another
- **WHEN** `model` is `openai/gpt-6-mini`, `model_roles.plan` is `anthropic/claude-opus`, and a Session in `plan` calls `plan_exit` and the user chooses `Approve and build`
- **THEN** the planning Turns ran on `anthropic/claude-opus` with `model_role: "plan"`, and the next Turn runs on `openai/gpt-6-mini` after an automatic `session.model.switched.1`

#### Scenario: Model switch during planning
- **WHEN** the planner is active and the user runs `/model openai/gpt-6`
- **THEN** the next plan-mode Turn runs on `openai/gpt-6`, and after the plan is approved the Session builds on `openai/gpt-6`

#### Scenario: Fallback outranks the planner
- **WHEN** the planner fails with `QuotaExceeded` and `fallback_models` is `["openai/gpt-6"]`
- **THEN** the Turn is re-sent on `openai/gpt-6`, which the Session uses until the fallback ends, in plan mode or not

#### Scenario: Subagents keep their model
- **WHEN** a subagent with `permission_mode: plan` starts while `model_roles.plan` is set
- **THEN** it runs on its own resolved model, not the planner
