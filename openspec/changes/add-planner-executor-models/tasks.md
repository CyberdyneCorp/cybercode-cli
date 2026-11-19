## 1. Selection

- [x] 1.1 `ModelRole::Plan` (`model_roles.plan`, no fallback); `cyber models` lists it; unit tests.
- [x] 1.2 The Turn selection (command model, fallback, planner, Session selection) used by the boundary and the automatic switch; top-level Sessions in `plan` only.
- [x] 1.3 An explicit model switch in `plan` suspends the planner until the next Mode change, folded from durable events.
- [x] 1.4 `model_role: "plan"` on `session.step.started.1` and the message entry.

## 2. Handoff

- [x] 2.1 `plan_exit` approval carries the approved plan text when the planner is active.
- [x] 2.2 Planner reasoning is dropped, not lowered, for a different model.

## 3. Surfaces

- [x] 3.1 The Session view's `model_role`; OpenAPI and SDK regenerated.
- [x] 3.2 The TUI footer `planner: <model>`, translated; screens test.
- [x] 3.3 Guide section and feature reference.

## 4. Tests

- [x] 4.1 Runtime tests with the fake provider: plan on the planner and build on the executor; unset role unchanged; `/model` during planning; subagents unaffected; planner reasoning not forwarded; `plan_exit` result carries the plan.

## 5. Later

- [ ] 5.1 Start the executor on a fresh user turn so Anthropic thinking can stay on after the handoff.
