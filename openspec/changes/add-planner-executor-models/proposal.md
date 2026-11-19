## Why

Planning and building ask different things of a model. A plan benefits from the strongest (and most expensive) model available; carrying out an approved plan is mostly mechanical and runs many more steps, where a cheaper model is good enough. Today a Session has one model: the user either pays for the strong model through the whole build or plans with the cheap one. Cyber already has plan mode (`plan_enter`, `plan_exit` with approval) and configurable model roles, so the split fits in without a new workflow.

## What Changes

- `provider-catalog` "Model roles": a new `plan` role in `model_roles`. Unset means disabled, so nothing changes for existing configurations.
- `provider-catalog` new requirement "Planner and executor models": while a top-level Session is in `plan` mode, its Turns run on `model_roles.plan` (the planner); once the Session leaves `plan` (an approved `plan_exit`, Shift+Tab, `/mode`), its Turns return to the Session's own model (the executor) through the existing automatic model switch. Precedence: a command's own model, then an active fallback, then the planner, then the Session's model. `/model` during plan mode wins over the planner for the rest of that plan phase and, as today, becomes the Session's model. `session.step.started.1` records `model_role: "plan"` for planner steps. The Session view reports `model_role`, and the TUI footer shows `planner: <model>`.
- `permissions-modes` "plan mode": when the planner model is active, an approved `plan_exit` returns the approved plan text in its tool result, so the executor receives the exact plan it is to carry out.
- `session-runtime` "Reasoning lowering across models": reasoning from planner steps is dropped, not lowered, when the history is projected for a different model, so the executor sees the plan, not the planner's hidden reasoning. Opaque reasoning state (signatures, encrypted reasoning) is already never sent to a model other than its producer.
- `configuration` "Model roles": lists `plan`.

## Non-goals

- A separate `execute` role. The executor is the Session's model (`model`, `model_roles.default`, `/model`), which already has every selection surface.
- Planner models for subagents, team members and workflow agents: they keep their own model resolution.
- A per-project `model_roles.plan`: like every other role, it is read from the global configuration.
- Planning without a user: `exec` cannot approve a plan (both plan tools are denied there), so it never hands off to an executor. `cyber exec --mode plan` runs on the planner.

## Impact

- `crates/cyber-llm`: `ModelRole::Plan`.
- `crates/cyber-server`: the Turn selection, the `/model` suspension, `model_role` on `session.step.started.1`, the message entry and the Session view, the reasoning projection.
- `crates/cyber-tools`: `plan_exit` carries the approved plan.
- `crates/cyber-tui`: the footer, translated.
- The OpenAPI document and the TypeScript SDK are regenerated.
- Docs: guide section, feature reference.
