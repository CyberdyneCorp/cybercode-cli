# routines Specification

## Purpose
A Routine is a saved Cyber Code job: a prompt, repositories, an agent, a model, connectors and an environment, run automatically when a trigger fires (a schedule, an API call, a GitHub event or a signed webhook). It brings Claude Code's routines and Codex automations to an open stack. Routines execute on any Runner (the local daemon, self-hosted pools or Cyber Cloud) and can launch plain Sessions, Goals or Workflow Runs.

## Requirements

### Requirement: Routine definition
(P3) A Routine SHALL have an ID with prefix `rtn_`, a name, a prompt template, one or more repositories (URL plus branch), an optional agent, model, mode ceiling (`plan`, `accept-edits`, `auto` or `dont-ask`; never `bypass`), MCP connectors, an Environment, a target (`local`, a pool, or `cloud`), a run kind (`session`, `goal` with condition, or `workflow` with workflow name and args), and a budget. Routines SHALL be stored by the Orchestrator for remote targets, and in the local database for `local` targets.

#### Scenario: Bypass rejected
- **WHEN** a Routine is created with mode ceiling `bypass`
- **THEN** creation fails with `InvalidRequestError` because unattended bypass is not allowed

### Requirement: Schedule triggers
(P3) A schedule trigger SHALL accept a 5-field cron expression and an IANA timezone (default the creator's timezone), or a one-time ISO-8601 timestamp. The minimum interval SHALL be 15 minutes for cloud targets and 1 minute for local targets.

#### Scenario: Nightly run
- **WHEN** a Routine has the trigger `0 3 * * *` in `America/Sao_Paulo`
- **THEN** it fires every day at 03:00 local time, observing daylight-saving changes

#### Scenario: Too frequent
- **WHEN** a cloud Routine is scheduled every 5 minutes
- **THEN** the trigger is rejected with the minimum interval in the error

### Requirement: API triggers
(P3) An API trigger SHALL expose `POST <orchestrator>/orch/v1/routines/{id}/fire` authenticated by a per-routine bearer token (shown once, rotatable). The request body SHALL be JSON of at most 64 KiB and SHALL be available to the prompt template as `{{payload}}` and its fields as `{{payload.<path>}}`. The response SHALL return the run ID.

#### Scenario: Fire from a deploy script
- **WHEN** a deploy script posts `{"version":"1.4.2"}` to the fire endpoint
- **THEN** a run starts whose prompt has `{{payload.version}}` replaced by `1.4.2`

### Requirement: GitHub triggers
(P3) A GitHub trigger SHALL subscribe to repository events through the Cyber GitHub App: `pull_request` (opened, synchronize, labeled), `issues` (opened, labeled), `release` (published), `check_suite` / `check_run` (completed with failure) and `push` to a branch pattern. Optional filters SHALL cover labels, base branch and author association. Event fields SHALL be available as `{{event.<path>}}`.

#### Scenario: Review every PR
- **WHEN** a PR is opened against `main` on a repository with a review Routine
- **THEN** a run starts with the PR number and diff URL in its prompt

### Requirement: Webhook triggers
(P3) A webhook trigger SHALL expose a unique URL that accepts POSTs signed with HMAC-SHA256 over the raw body using a per-trigger secret (header `X-Cyber-Signature: sha256=<hex>`). It SHALL reject unsigned or mis-signed requests with `401`, and SHALL reject timestamps (`X-Cyber-Timestamp`) older than 5 minutes.

#### Scenario: Forged webhook
- **WHEN** a request arrives with an invalid signature
- **THEN** it is rejected with `401` and no run starts

### Requirement: Combined triggers
(P3) A Routine SHALL accept multiple triggers of any kind. Each run SHALL record which trigger fired it.

#### Scenario: Nightly and on-demand
- **WHEN** a Routine has both a nightly schedule and an API trigger
- **THEN** both fire runs, and run history shows `trigger: schedule` or `trigger: api`

### Requirement: Runs as sessions
(P3) Each run SHALL create a Session (ID linked to the run ID `run_…`) on the target Runner, with the Routine's agent, model, mode ceiling and Environment. For kind `goal` it SHALL set the goal condition before the first Turn. For kind `workflow` it SHALL start the named Workflow Run with the rendered args.

#### Scenario: Goal routine
- **WHEN** a Routine of kind `goal` with condition "all lint errors fixed" fires
- **THEN** the Session starts with that active goal and continues until the evaluator reports it met or impossible, or the budget is exhausted

### Requirement: Local target via daemon
(P3) Routines with target `local` SHALL run on the user's `cyber service` daemon. If the machine is asleep at fire time, the run SHALL start on wake when it is less than `routines.catch_up_window` (default 2 hours) late, and SHALL otherwise be recorded as `missed`.

#### Scenario: Laptop asleep
- **WHEN** a local Routine was due 30 minutes ago and the laptop wakes
- **THEN** the run starts immediately with `late_by: 30m` recorded

### Requirement: Overlap policy
(P3) Each Routine SHALL define `overlap` as `skip` (default: do not start while a previous run is active), `queue` (start after the previous run ends, at most 1 queued) or `parallel` (up to `max_parallel`, default 3).

#### Scenario: Long run with skip
- **WHEN** a schedule fires while the previous run is still active and `overlap` is `skip`
- **THEN** the new run is recorded as `skipped` and not started

### Requirement: Budgets
(P3) Each Routine SHALL have a per-run budget (`max_turns` default 200, `max_cost_usd` default 5, `max_duration` default 2 hours). A run exceeding a budget SHALL be interrupted at the next Safe Boundary with status `budget_exceeded` and SHALL notify the owner.

#### Scenario: Cost cap
- **WHEN** a run reaches its `max_cost_usd`
- **THEN** it stops at the next Safe Boundary with status `budget_exceeded`

### Requirement: Run history and logs
(P3) `cyber routines runs <rtn_id>` SHALL list runs with status (`running`, `succeeded`, `failed`, `budget_exceeded`, `skipped`, `missed`, `cancelled`), trigger, start time, duration, cost and resulting PRs. Each run SHALL link to its Session, which can be opened, steered or teleported like any remote Session.

#### Scenario: Inspect a failed run
- **WHEN** the user opens a failed run
- **THEN** they can read the full Session transcript and teleport it locally

### Requirement: Notifications
(P3) Routine owners SHALL be notified on run failure and budget exhaustion, and optionally on success, via push (remote-control Devices), email through the account, or a configured channel.

#### Scenario: Failure push
- **WHEN** a nightly run fails
- **THEN** the owner's phone receives a push naming the Routine and the failure status

### Requirement: CLI and slash command
(P3) The system SHALL provide `cyber routines create|list|show|edit|fire|pause|resume|rm|runs`, and the TUI command `/schedule`, which creates a Routine interactively from the current Session's repository, agent and prompt.

#### Scenario: Create from a session
- **WHEN** the user runs `/schedule every weekday at 9 review open PRs`
- **THEN** a Routine draft with a cron trigger `0 9 * * 1-5` is shown for confirmation and saved on approval

### Requirement: Enable and disable
(P3) `cyber routines pause <id>` SHALL stop all triggers without deleting the Routine, and `resume` SHALL re-enable them. An org admin toggle `policy.routines.enabled: false` SHALL stop all members' Routines and block creation.

#### Scenario: Org disables routines
- **WHEN** an org admin disables routines
- **THEN** existing member Routines stop firing and creation fails with `ForbiddenError`

### Requirement: Ownership and credentials
(P3) A run SHALL act with the owner's Cyber Account identity and the Routine's scoped git credentials. If the owner loses the `cyber-code` entitlement or leaves the org, the Routine SHALL pause automatically and notify the owner.

#### Scenario: Owner leaves org
- **WHEN** the owner is removed from the org
- **THEN** the org's Routines owned by them pause, and an admin can transfer ownership

### Requirement: Routines API
(P3) The Orchestrator SHALL expose `GET|POST /orch/v1/routines`, `GET|PATCH|DELETE /orch/v1/routines/{id}`, `POST .../fire`, `POST .../pause|resume`, `GET .../runs`, and `POST .../tokens/rotate`. The local server SHALL proxy these at `/api/v1/routines` for clients.

#### Scenario: SDK creates a routine
- **WHEN** an SDK client posts a Routine definition to `/api/v1/routines`
- **THEN** it is validated and stored, and its ID is returned
