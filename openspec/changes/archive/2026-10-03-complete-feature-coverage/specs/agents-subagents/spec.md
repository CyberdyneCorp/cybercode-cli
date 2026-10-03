## MODIFIED Requirements

### Requirement: Agent tool spawns subagents
(P1) The system SHALL provide an `agent` tool with inputs `prompt` (required), `agent` (default `general`), `description` (3–8 words), `output_schema` (JSON Schema), `model`, `isolation` (`none`, `worktree` or, from P3, `remote`), `runner` (with `isolation: remote`: a pool, `rnr_` ID or peer name), `background` (boolean), `fork` (boolean) and `resume` (subagent name or `ses_` ID). Each spawn SHALL create a child Session whose `parent_id` is the caller, titled `<description> (@<agent>)`, and SHALL request the `agent` permission with the target agent name as resource.

#### Scenario: Foreground subagent
- **WHEN** the model calls `agent` with `agent: "explore"` and `background: false`
- **THEN** a child Session runs to completion and the tool result contains the subagent's final text and its `ses_` ID

#### Scenario: Unknown agent
- **WHEN** the model calls `agent` with `agent: "nonexistent"`
- **THEN** the call fails with `Unknown agent "nonexistent". Available: <names>` without creating a Session

## ADDED Requirements

### Requirement: Remote isolation
(P3) A spawn with `isolation: "remote"` SHALL create the child Session on the named `runner` (or `runners.default`) as defined by `runners-cloud`: the repository is materialized there, the child runs in its own worktree, and the handback carries the branch and diff summary plus a `runner` field. Permission requests from a remote child SHALL surface in the parent's client like local ones. The parent's Mode ceiling and deny rules SHALL apply on the Runner. Without a Cyber Account or peer able to host the Session, the spawn SHALL fail with `RunnerUnavailable`.

#### Scenario: Offload a long test run
- **WHEN** the model spawns `general` with `isolation: "remote"`, `runner: "ci-pool"` and `background: true`
- **THEN** the child runs on the pool, and when it finishes the parent receives a queued handback with its result, branch and `runner: "ci-pool"`
