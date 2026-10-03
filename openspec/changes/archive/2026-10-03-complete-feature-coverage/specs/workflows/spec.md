## ADDED Requirements

### Requirement: Remote agents
(P3) `agent()` SHALL accept `runner` (a pool name, an `rnr_` Runner ID, or a configured peer name). The child Session SHALL be created on that Runner or peer through the Orchestrator or peer API, with the repository materialized as for `cyber --cloud` and `isolation` defaulting to `worktree` there. Results SHALL be persisted as `workflow.agent.settled.1` exactly as for local agents, counted against the run's Budget, shown in the run monitor with the Runner name, and resumable after a restart of either side. A `runner` that is unavailable SHALL reject the call with `RunnerUnavailable` naming it.

#### Scenario: Fan out across machines
- **WHEN** a script launches 20 agents with `runner: "gpu-pool"` and 4 with `runner: "desk"`
- **THEN** the pool executes 20 child Sessions and the peer `desk` executes 4, and the monitor shows each agent's Runner

### Requirement: Automatic orchestration
(P2) `workflows.auto` SHALL accept `off`, `suggest` (default) and `on`. When not `off`, the system prompt SHALL instruct the model to draft a Workflow (inline script through the `workflow` tool) for tasks it judges substantive: several independent parts, more than `workflows.auto_threshold_files` (default 10) files, or a review or research task with several angles. In `suggest`, the draft, agent count and cost estimate SHALL be shown and require approval through the `workflow.run` permission; in `on` the run starts under that permission's rules. `workflows.size_guideline` (default 10 agents) SHALL be given to the model, and a draft above it SHALL carry a warning in the approval prompt. `/orchestrate <task>` SHALL force a draft for one task.

#### Scenario: Large migration suggested as a workflow
- **WHEN** `workflows.auto` is `suggest` and the user asks to migrate 300 files to a new API
- **THEN** the model proposes a `migrate` workflow with its agent count and estimated cost, and nothing runs until the user approves
