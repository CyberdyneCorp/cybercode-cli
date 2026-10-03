# runners-cloud Specification

## Purpose
Runners execute Cyber Code Sessions somewhere other than the user's terminal: on the user's own always-on machine, on self-hosted infrastructure, or on Cyber Cloud. Work then continues after the laptop closes, and many tasks can run in parallel. It brings Claude Code's cloud sessions and self-hosted environments (`--cloud`, `--teleport`, auto-fix PRs) and Codex Cloud environments to an open, self-hostable orchestrator. It adds handoff of a live local Session to a Runner, which neither product offers.

## Requirements

### Requirement: Runner types
(P3) The system SHALL support three Runner kinds: `local` (a user's own machine registered through the Relay), `self-hosted` (a machine in a pool registered with an Orchestrator) and `cloud` (Cyber Cloud-managed). Each Runner SHALL have an ID with prefix `rnr_`, a pool name, labels, capacity and status (`online`, `draining`, `offline`).

#### Scenario: List runners
- **WHEN** the user runs `cyber runners list`
- **THEN** it prints each reachable Runner with kind, pool, labels, status and free capacity

### Requirement: Orchestrator
(P3) The system SHALL provide `cyber orchestrator serve --listen <addr> --db <postgres url> --issuer <discovery url>` exposing the Orchestrator API (`/orch/v1`), which queues Sessions, assigns them to Runners by pool and labels, and stores run metadata. Cyber Cloud SHALL run the same Orchestrator. Every Orchestrator call SHALL be authenticated by a Cyber Account token with scope `cyber:runner`, or by a Runner registration token.

#### Scenario: Self-hosted orchestrator
- **WHEN** an org deploys `cyber orchestrator serve` and sets `policy.runners.orchestrator_url`
- **THEN** members' `--cloud` sessions are queued on the org's orchestrator instead of Cyber Cloud

### Requirement: Self-hosted runner registration
(P3) `cyber runner start --pool <name> --orchestrator <url> --token <registration token>` SHALL register the machine and then poll for work over an outbound connection. It SHALL send a heartbeat every 15 seconds and SHALL be marked `offline` after 3 missed heartbeats. Registration tokens SHALL be created by org admins, scoped to one pool, and revocable.

#### Scenario: Runner disappears
- **WHEN** a Runner misses 3 heartbeats
- **THEN** it is marked `offline` and its running Sessions are marked `interrupted`, resumable on another Runner

### Requirement: Runner draining
(P3) `cyber runner drain` SHALL stop the Runner from accepting new Sessions, let running Sessions finish or reach `runners.drain_timeout` (default 30 minutes), then exit. Sessions still running at the timeout SHALL be checkpointed and requeued.

#### Scenario: Upgrade a runner
- **WHEN** an operator drains a Runner before upgrading it
- **THEN** no new Sessions are assigned and running ones complete or are requeued with their checkpoint

### Requirement: Environments
(P3) An Environment SHALL define a container image or machine template, a setup script, environment variables, secrets references, a network level (`none`, `allowlist` with domain list, or `full`), resource limits and a cache policy. Setup results SHALL be cached as layers keyed by image digest and a setup-script hash, for up to 7 days. Environments SHALL be managed with `cyber env create|list|edit|rm`.

#### Scenario: Cached setup
- **WHEN** two Sessions start with the same Environment and unchanged setup script
- **THEN** the second Session reuses the cached setup layer and skips the setup script

#### Scenario: No network
- **WHEN** an Environment uses network level `none`
- **THEN** commands in the Session cannot reach external hosts except the Orchestrator and the configured model providers

### Requirement: Secrets handling
(P3) Environment secrets SHALL be stored encrypted at rest by the Orchestrator, injected only into the Session's sandbox, masked in transcripts and logs (replaced by `****`), and never sent to model providers as part of tool output when masking is possible.

#### Scenario: Secret echoed
- **WHEN** a command prints the value of a secret
- **THEN** the transcript and the model see `****` instead of the value

### Requirement: Start a remote session
(P3) `cyber --cloud "<task>" [--env <name>] [--pool <name>] [--repo <url>]` SHALL create a new Session on a Runner for the current repository, or for `--repo`, and print its ID and URL. The local terminal SHALL stay free. The same SHALL be possible from the TUI (`/cloud <task>`), the web client, the mobile app and the SDK.

#### Scenario: Start from the terminal
- **WHEN** the user runs `cyber --cloud "fix the flaky auth test"` in a git repo
- **THEN** a Session starts on a Runner with the current branch pushed or bundled, and the command returns immediately with the Session ID

### Requirement: Repository materialization
(P3) Starting a remote Session SHALL make the repository available on the Runner by cloning from the remote with a scoped credential. Local commits not on the remote SHALL be shipped as a git bundle, and uncommitted changes as a patch applied on top. The Runner SHALL verify that the resulting tree hash matches the local tree hash.

#### Scenario: Unpushed work
- **WHEN** the user starts a remote Session with 2 unpushed commits and uncommitted edits
- **THEN** the Runner's working tree matches the local tree exactly

### Requirement: Live handoff to a runner
(P3) `cyber handoff [--session <id>] [--env <name>]` SHALL move a live local Session to a Runner. It SHALL interrupt the local Drain at a Safe Boundary, bundle the durable event history, inbox, Context Epoch, goal state, snapshots and repository state, recreate the Session on the Runner with the same ID, and resume it there. The local copy SHALL become a read-only mirror that follows the remote Session through the Relay.

#### Scenario: Close the laptop mid-task
- **WHEN** the user runs `cyber handoff` during a long refactor with an active goal
- **THEN** the Session continues on the Runner with the same history and goal, and the terminal shows it mirrored

### Requirement: Teleport a remote session locally
(P3) `cyber teleport <ses_id>` SHALL fetch a remote Session's branch and full history into the current repository, recreate the Session locally with the same ID, and mark the remote copy `transferred`. It SHALL refuse to run when the local working tree has uncommitted changes, unless `--stash`.

#### Scenario: Continue locally
- **WHEN** the user teleports a finished cloud Session
- **THEN** the branch is checked out locally and `cyber -s <ses_id>` resumes it with full history

### Requirement: Session identity token
(P3) Each remote Session SHALL receive a short-lived JWT (`CYBER_SESSION_TOKEN`, 15-minute TTL, auto-refreshed) signed by the Orchestrator with claims `sub` (account), `ses`, `rnr`, `org` and `repo`. The Orchestrator SHALL publish its JWKS at `/orch/v1/.well-known/jwks.json` so internal services can verify requests that come from Sessions.

#### Scenario: Internal service check
- **WHEN** a Session calls an internal API presenting `CYBER_SESSION_TOKEN`
- **THEN** the API verifies it via the Orchestrator JWKS and sees which account and repo the request comes from

### Requirement: Git credentials
(P3) Runners SHALL obtain git credentials as short-lived, repository-scoped installation tokens from the Cyber GitHub App (or a configured GitLab or Gitea integration), minted per Session with the minimum permissions requested by the Session's mode (read-only for `plan`). Tokens SHALL never be written to the transcript.

#### Scenario: Plan-mode session
- **WHEN** a remote Session runs in `plan` mode
- **THEN** its git token has read-only `contents` permission and pushes fail

### Requirement: Auto-fix pull requests
(P3) A remote Session that opened a pull request SHALL be able to watch it (`/autofix on`). CI failures, review comments and requested changes SHALL be admitted into the Session as prompts with `delivery: "queue"` and `origin: { kind: "vcs" }`, so the agent can push fixes, until the PR merges or closes, or after 7 days.

#### Scenario: CI fails
- **WHEN** a check run fails on the watched PR
- **THEN** the failure summary and log excerpt are admitted to the Session, which starts a Turn to fix it

### Requirement: Concurrency and quotas
(P3) Pools SHALL enforce `max_concurrent_sessions` (default per Runner = CPU cores / 2). Orgs SHALL be able to set per-member concurrent-session and monthly runner-minute quotas derived from entitlements (`cyber-code:pro`, `cyber-code:team`). Excess Sessions SHALL wait in a queue, and their position SHALL be visible to the user.

#### Scenario: Quota exhausted
- **WHEN** a member at their concurrent-session limit starts another cloud Session
- **THEN** the Session is queued with status `queued` and position shown

### Requirement: Runner logs and metrics
(P3) `cyber runners logs <rnr_id|ses_id>` SHALL stream Runner and Session logs. Runners and the Orchestrator SHALL expose Prometheus metrics at `/metrics` including `cyber_runner_sessions_active`, `cyber_runner_queue_depth`, `cyber_runner_setup_seconds` and `cyber_runner_heartbeat_failures_total`.

#### Scenario: Scrape metrics
- **WHEN** Prometheus scrapes a self-hosted Runner
- **THEN** it receives the active session gauge and setup duration histogram

### Requirement: Data residency
(P3) For `self-hosted` and `local` Runners, repository contents, tool output and transcripts SHALL stay on the Runner and the org's Orchestrator. Only routing metadata and, for hosted relays, encrypted traffic SHALL leave. Model traffic SHALL go only to the providers configured in the Session.

#### Scenario: Compliance review
- **WHEN** an org runs a self-hosted Orchestrator and Runners with its own model gateway
- **THEN** no repository content is sent to Cyber Cloud

### Requirement: Runner selection
(P3) `cyber runners use <pool|rnr_id>` SHALL set the default target for `--cloud` and `handoff`, in user state or as `runners.default` in config. Policy MAY restrict allowed pools via `policy.runners.allowed_pools`.

#### Scenario: Restricted pool
- **WHEN** a member selects a pool not in `policy.runners.allowed_pools`
- **THEN** the command fails with `ForbiddenError` listing the allowed pools

### Requirement: Remote session lifecycle
(P3) Remote Sessions SHALL support the full Session API through the Orchestrator proxy. Their status SHALL be one of `queued`, `provisioning`, `running`, `idle`, `interrupted`, `transferred`, `archived`. Idle remote Sessions SHALL release their Runner after `runners.idle_release` (default 30 minutes) and be reprovisioned on the next prompt from their checkpoint.

#### Scenario: Prompt after release
- **WHEN** the user prompts a remote Session idle for 2 hours
- **THEN** it is reprovisioned on a Runner from its checkpoint and continues

### Requirement: Audit
(P3) The Orchestrator SHALL keep an audit log of Session creation, handoff, teleport, credential minting, secret access and Runner registration, retained for 365 days by default, and SHALL export it via `/orch/v1/audit`.

#### Scenario: Who minted a token
- **WHEN** an admin queries the audit log for a repository
- **THEN** each git-credential minting event lists the account, Session and Runner

### Requirement: Exclusive execution ownership during transfer
(P3) Every transferable Session SHALL have an authoritative owner and monotonically increasing ownership epoch. Transfer SHALL use durable states `preparing`, `ready`, `committed` and `aborted` with a unique transfer ID. The source SHALL quiesce tools and background mutations and persist its execution prohibition before sending state. The destination SHALL verify the manifest before the ownership authority atomically commits the new owner and epoch; it SHALL not execute earlier. Dispatch and hosted state writes SHALL require the current epoch. A source without confirmation SHALL remain paused and query the authority; a timeout SHALL NOT authorize local resume. A partitioned Runner SHALL stop new dispatch when its 30-second lease expires. Failover SHALL wait for prior lease expiry and reconcile in-flight external effects; fencing SHALL NOT be claimed to undo an already dispatched external action.

#### Scenario: Acknowledgement lost after handoff
- **WHEN** the destination becomes owner but the source loses the acknowledgement
- **THEN** the destination may execute, the source remains paused, and reconnect discovers the committed epoch without a second Drain
