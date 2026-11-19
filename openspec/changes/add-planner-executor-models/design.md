## Context

A Session's model is chosen per Turn at the Safe Boundary: `boundary()` resolves the effective selection (a command's own model, else an active fallback, else the Session's `ModelSelection`), and `automatic_model_switch` records `session.model.switched.1 { automatic: true }` whenever the resolved reference differs from `info.model`. Automatic switches move `info.model` without touching the Session's own selection, which is what lets a fallback expire back to the configured model. Plan mode is a Mode: `plan_enter` and `plan_exit` switch it at the next Turn, and `plan_exit` asks the user to approve.

The planner/executor split reuses both: the planner is one more input to the Turn's selection, and the existing automatic switch does the rest.

## Goals / Non-Goals

**Goals:** plan on one model and build on another with one config key; no change when the key is unset; the executor gets the approved plan and nothing of the planner's hidden reasoning; usage stays attributable per role; the user can always see which model runs.

**Non-Goals:** an `execute` role, planner models for child Sessions, per-project roles, unattended plan approval.

## Decisions

1. **Config shape: `model_roles.plan`.** A new role in the existing map, with no fallback: unset disables the planner, like `advisor`. It is read from the global configuration, like every role (the resolver holds the global configuration), so a cloned repository cannot redirect planning to another provider. Workflow tiers do not accept `role:plan`; the role only means something to a Session in plan mode.
2. **The executor is the Session's own model.** No `execute` role: `model`, `model_roles.default` and `/model` already pick the model a Session builds with, and a second name for it would add a precedence question without adding a capability.
3. **When the planner applies.** At each Safe Boundary of a Session that has no parent, whose Mode is `plan`, and whose plan phase was not overridden by `/model` (below). Subagents, team members and workflow agents keep their own model resolution, so an agent's pinned model is never replaced.
4. **Precedence.** A command's own model (a skill or command with `model`), then an active fallback, then the planner, then the Session's selection. A fallback already means "the configured model is failing"; it keeps its own lifetime (`fallback.sticky_minutes` or the user's next `/model`), whichever phase it started in.
5. **Handoff.** Leaving `plan` (an approved `plan_exit`, Shift+Tab, `/mode`, the API) changes the Mode at the next Turn; that Turn's boundary no longer selects the planner, so `automatic_model_switch` returns `info.model` to the Session's model. Entering `plan` does the reverse. No new event type: both are `session.model.switched.1 { automatic: true }`, and `PreModelSwitch`/`PostModelSwitch` observe them like any automatic switch.
6. **What the executor receives.** The whole visible conversation, as today, plus the approved plan text in the `plan_exit` tool result (`Plan approved. … Approved plan (<path>):` followed by the file's content) when the planner was the Turn's model. The planner's reasoning is not forwarded: when history is projected for a different model, reasoning items of planner steps are dropped instead of lowered to assistant text. Opaque state (Anthropic signatures, Gemini thought signatures, OpenAI encrypted reasoning) already goes back only to the model that produced it, so nothing crosses providers. Without a planner the tool result and the reasoning projection are unchanged.
7. **`/model` in plan mode.** An explicit model switch while the Session is in `plan` suspends the planner until the Mode next changes, and, as today, becomes the Session's model (so the executor too). This keeps `/model` meaning "the Session's model" everywhere and makes a switch take effect immediately rather than being undone by the next boundary. Re-entering plan mode uses the planner again.
8. **Usage attribution.** `session.step.started.1` carries `model_role: "plan"` on steps the planner ran (absent otherwise), and the message entry exposes it. Cost and tokens already split by model in the usage ledger, `/usage` and OpenTelemetry metrics, since planner and executor are different models; no ledger or metric schema changes, so no new metric cardinality. Budgets count both roles, as they count every model.
9. **Visibility.** The Session view gains `model_role` (`"plan"` while the planner is the active model, absent otherwise). The TUI footer shows `planner: <model>` in the accent color while it is set; the existing model field keeps showing the active model, and the model switch is recorded in the history like any automatic switch.
10. **`exec`.** Non-interactive Sessions cannot call `plan_enter` or `plan_exit`, so they never hand off. `cyber exec --mode plan` runs its Turns on the planner (it is a plan-mode Session) and ends there.

## Risks / Trade-offs

- **Anthropic thinking after the handoff.** An Anthropic executor with thinking on continues the tool loop from the planner's `plan_exit` call, which has no native thinking block, so the existing thinking guard turns thinking off until the next user message. This is the Messages API's rule for any mid-loop model switch, not new here. Mitigation left open (below).
- **A fallback started by the planner outlives the plan phase** for at most `fallback.sticky_minutes`. Accepted: tying fallbacks to phases would need more state for a rare case, and the user can end it with `/model`.
- **Same model, different variants** (for example `p/m#high` planning, `p/m#low` building) share one row in per-model usage. `model_role` on the step still tells them apart.

## Open Questions

- Should the handoff start the executor on a fresh user turn (so thinking can stay on for Anthropic executors)? That changes the shape of the conversation for every provider, so it is left for a follow-up.
- Per-project `model_roles.plan` behind workspace trust, if users ask for it.
