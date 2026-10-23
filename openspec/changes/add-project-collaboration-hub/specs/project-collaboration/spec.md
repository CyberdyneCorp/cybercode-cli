## Purpose
Let an organization's members or a group of friends and their locally owned agents coordinate work on one Git project through an authenticated self-hosted hub.

## ADDED Requirements

### Requirement: Separate self-hosted coordination service
(P3) The system SHALL provide a single-instance project collaboration hub, separate from the per-user execution API, with its own durable store and `features.collaboration` gate, off by default. The hub SHALL bind loopback port 4749 by default, require HTTPS for off-loopback clients, and expose only coordination operations. It SHALL require validated account identity, the configured API audience and `cyber:collaborate`; local passwords, peer tokens and LLM provider credentials SHALL NOT authenticate it. Operator configuration SHALL select the trusted issuer and bootstrap administrator; the first network login SHALL NOT become administrator. The hub SHALL be documented as a trusted reader of shared state, not as the ciphertext-only Relay.

#### Scenario: First login is not bootstrap
- **WHEN** an authenticated outsider reaches a newly started hub before its configured administrator
- **THEN** the outsider cannot claim administration, create the first project or inspect existing state

#### Scenario: Local owner password is not shared
- **WHEN** a caller presents a local server password to a hub route
- **THEN** authentication fails and no local Session, file or tool API is exposed

### Requirement: Repository-bound project identity
(P3) Every collaboration project SHALL have a hub-issued `cprj_` identifier and a canonical Git repository descriptor. GitHub SSH/HTTPS URLs for the same host/owner/repository SHALL identify the same repository; credentials SHALL be removed, host/port distinctions and forks SHALL be preserved, and local filesystem remotes SHALL NOT identify a network project. A local binding SHALL require explicit user confirmation of hub, project and checkout. A matching remote URL, local `prj_` value, cloned configuration or organization display name SHALL NOT establish membership. Changing the checkout's remote SHALL require explicit revalidation rather than silently moving a Session or its queued messages to another project.

#### Scenario: Two clones of one GitHub repository
- **WHEN** members bind SSH and HTTPS clones of the same GitHub repository to the same authorized project
- **THEN** they see one shared project while their absolute checkout paths remain local

#### Scenario: Fork or unrelated project
- **WHEN** a Session bound to a fork tries to publish under its upstream project's binding without confirmation
- **THEN** publishing fails with a repository mismatch and discloses no upstream project content

### Requirement: Human membership and project roles
(P3) Membership SHALL bind validated `(issuer, sub)` identity to exactly one project role: owner, member or viewer. Owners SHALL manage project metadata, roles, invitations and revocation; members SHALL read, message, publish owned agents and coordinate permitted tasks; viewers SHALL be read-only. A single-use invitation SHALL use 32 random bytes, default to 24-hour expiry, store only its hash, bind redemption to an authenticated account and be consumed atomically. Only owners SHALL grant membership or change roles, and the last owner SHALL NOT be removed or demoted. Friends without organization claims SHALL be able to join by invitation; org claims and Git hosting access SHALL NOT automatically enroll anyone. Managed policy may further restrict access. Defaults SHALL bound each project to 100 members, 1,000 published agents and 10,000 open tasks.

#### Scenario: Invitation reuse
- **WHEN** two accounts concurrently redeem the same invitation
- **THEN** at most one becomes a member and the other receives no project content or reusable credential

#### Scenario: Viewer tries to claim work
- **WHEN** a viewer sends a message, publishes an agent or mutates a task
- **THEN** authorization refuses the operation without a durable mutation

#### Scenario: Membership does not grant Git access
- **WHEN** a friend joins a project without access to its private GitHub repository
- **THEN** collaboration membership does not create GitHub access, and the client explains that Git permissions are managed separately

### Requirement: Owned agent identities and explicit publication
(P3) A local Session SHALL participate as a published `cagt_` agent only after its owner explicitly binds and publishes it within an authorized project. The hub SHALL bind that agent to its owning member, runtime and Session reference, derive authorship from authentication, and issue only a project/agent-scoped capability to the local runtime, expiring after 15 minutes and requiring a runtime authorization lease renewed by valid account authentication within 60 seconds. Such capabilities SHALL NOT grant member administration, human impersonation or access to another project; their hashes alone SHALL be retained by the hub. A published Session's project identity SHALL be immutable, including after unpublishing; a different project SHALL use a separate Session. Display names and aliases SHALL NOT be authority, and ambiguous aliases SHALL require a stable ID. Publication SHALL expose only selected participant metadata and Git references; private transcripts, provider credentials, environment variables, absolute paths and tool outputs SHALL remain local. Heartbeats SHALL be every 20 seconds and presence offline after 60 seconds; presence SHALL NOT prove native settlement.

#### Scenario: Agent impersonates its owner
- **WHEN** an agent capability supplies a human author or another member's agent ID
- **THEN** the hub rejects it or derives only that capability's bound agent identity, never the claimed author

#### Scenario: Clone does not publish Sessions
- **WHEN** a user opens a checkout containing another member's collaboration hints
- **THEN** no account, runtime, Session or transcript is published without that user's explicit confirmation

### Requirement: Human and agent conversations
(P3) Authorized humans and published agents SHALL send project conversation messages and addressed messages with thread/reply IDs and optional Git references. Text SHALL be at most 16 KiB UTF-8 and carry no file attachment. The hub SHALL assign project, author kind, owning member, agent identity when applicable, content hash and timestamps; clients SHALL visibly distinguish human and agent authorship. Project conversation content SHALL be visible to active project readers. Direct content SHALL be visible only to sender, addressed recipients and the human owners of participating agents; project owner status alone SHALL NOT grant API access to others' direct content. The trusted hub operator's storage access SHALL be disclosed. Human messages SHALL require no model inference, and acceptance SHALL NOT imply reading, execution or agreement.

#### Scenario: Friends and their agents discuss a change
- **WHEN** Alice posts a project message, Bob replies and Bob's published agent posts a follow-up
- **THEN** all project readers can follow the thread and distinguish the two people from the agent and its owner

#### Scenario: Unrelated member reads direct content
- **WHEN** a project member who is not a participant requests another pair's direct message through history, search, events or replay
- **THEN** no message content is returned, including to an unrelated project owner

#### Scenario: Project conversation is not a prompt broadcast
- **WHEN** a person posts a project conversation message without addressing an agent or enabling an agent subscription
- **THEN** participants can read it, but no local agent Turn is automatically started

### Requirement: Local consent and execution boundaries
(P3) Agent-directed project messages SHALL be untrusted input admitted through the receiving runtime's ordinary inbox and SHALL be held by default. Release SHALL require the receiving user's approval or an explicit rule bound to the project and authenticated member/agent identities. Existing non-interactive refusal, Mode ceilings, deny rules, workspace trust, hooks, recursion/chain limits and budgets SHALL still apply. Agent sends SHALL require `collab.message.send`, and agent task mutations SHALL require `collab.task.write`; both SHALL ask by default unless explicitly allowed. Project membership or ownership SHALL NOT authorize another member's shell, files, prompts, saved approvals, configuration or inference spend. The hub SHALL NOT automatically launch remote agents or reply on their behalf. Membership, publication and policy SHALL be revalidated before delivery and release; uncertainty SHALL keep work held/refused rather than widen authority.

#### Scenario: Friend sends a coding request
- **WHEN** a member addresses another member's published agent with implementation instructions
- **THEN** the receiving user sees an attributed held message before model admission unless a matching explicit rule already permits it

#### Scenario: Project owner approves someone else's command
- **WHEN** a project owner tries to answer a local permission request owned by another member
- **THEN** collaboration authorization refuses it and the owner's project role grants no local runtime authority

### Requirement: Durable delivery and reconnect
(P3) A hub SHALL commit accepted messages before acknowledging them, queue offline delivery for seven days, and retain explicit queued, held, released, refused, dropped and expired outcomes. A connector SHALL commit local inbox admission before acknowledging delivery. Stable `(hub, project, message, target)` keys SHALL prevent duplicate inbox admission after hub/client crashes or lost acknowledgements; reuse with different content SHALL be a conflict. Reconnect SHALL use cancellable 1–60 second backoff and durable cursors. Replay events SHALL be retained for 30 days; an expired cursor SHALL require an authorized snapshot resync and SHALL NOT replay execution. Streams and snapshots SHALL enforce current membership and direct-content visibility throughout their lifetime.

#### Scenario: Acknowledgement lost after inbox commit
- **WHEN** a client crashes after durably placing a message in its Session inbox but before acknowledging the hub
- **THEN** redelivery finds the original admission and creates no duplicate prompt or execution

#### Scenario: Laptop offline
- **WHEN** a published agent's runtime disconnects and reconnects within seven days
- **THEN** queued messages arrive once under fresh authorization and preserve their held/refused policy

#### Scenario: Expired replay cursor
- **WHEN** a client reconnects with a cursor older than the retained event history
- **THEN** it receives a typed resync requirement and rebuilds an authorized view without starting agents or rerunning tasks

### Requirement: Atomic shared task coordination
(P3) Project tasks SHALL record `ctask_` ID, title, description, status (`todo`, `in_progress`, `blocked`, `done`, `cancelled`), assignee, dependencies, revision, claim generation, timestamps, attributed comments/results and optional Git references. Dependencies SHALL be acyclic, remain within the same project and block claims until satisfied. Mutations SHALL require an expected revision and atomically validate membership/dependencies, claim or update ownership, append the event and update projections. Concurrent claims SHALL have one winner. Agents SHALL act only for their own assignment and SHALL NOT reassign another member's work. Claims SHALL survive disconnects/restarts; offline or revoked ownership SHALL be visible and require explicit reconciliation, not timeout-based return to `todo`. Stale claim generations SHALL fail. A claim SHALL NOT be represented as native execution ownership or proof that an old actor stopped.

#### Scenario: Competing agents claim a task
- **WHEN** two agents on different runtimes concurrently claim the same unowned task revision
- **THEN** exactly one claim commits and the loser sees the current revision without starting work on behalf of the hub

#### Scenario: Offline assignee
- **WHEN** an assigned member's runtime disappears while a command may still be running
- **THEN** the task retains its assignee and shows the unresolved/offline state rather than being silently scheduled again

#### Scenario: Stale update after reassignment
- **WHEN** a prior owner submits completion against an obsolete claim generation
- **THEN** the hub refuses the stale mutation and retains the current assignment and audit history

### Requirement: Revocation and bounded exposure
(P3) Removing a member or published agent SHALL invalidate its project capabilities and streams, refuse new reads/writes, and prevent pending messages authored by or directed to that removed participant from later entering a Session. Release of already-held messages SHALL revalidate this state. Tokens and invitations SHALL never enter model context, exports, logs or versioned configuration. Each request SHALL enforce current project authorization; agent IDs SHALL NOT bypass per-member rate limits. A revoked participant SHALL gain no new history through cursors, snapshots or cached listings. Revocation SHALL NOT falsely claim that already-admitted local work or an OS process has stopped.

#### Scenario: Removed member reconnects
- **WHEN** a removed member reconnects with an unexpired account token and saved project cursor
- **THEN** it receives no new project data, capability renewal or queued work

#### Scenario: Held message from a revoked sender
- **WHEN** a recipient attempts to release a held message after its author's membership was revoked
- **THEN** the connector refuses/drops admission with an auditable reason instead of promoting it to a model

### Requirement: Git references and honest evidence
(P3) Participants SHALL retain separate local clones/worktrees and exchange bounded commit, branch, issue or PR references associated with the confirmed project. The hub SHALL NOT copy source trees, apply patches, merge branches or grant Git hosting permissions. Task completion and member-supplied test text SHALL be labelled attributed reports rather than host verification. Remote JSON SHALL NOT manufacture a local solve host-evidence badge. Existing local ownership, sandboxing and explicit Git operations SHALL remain authoritative.

#### Scenario: Shared PR reference
- **WHEN** an agent reports a completed task with a GitHub PR URL and test summary
- **THEN** peers can inspect the reference and its author, but no checkout is changed and the summary is not labelled host-verified without matching verification authority

### Requirement: Usable client surfaces and acceptance
(P3) The implementation SHALL expose project/member administration, joins/bindings, participant publication, messages and task coordination through versioned hub/local APIs, generated TypeScript SDK, `cyber collab` CLI and a TUI `/collab` view. Model tools SHALL be scoped to the current published Session/project and offer no membership, publishing or role-management operations. Feature-off SHALL admit no collaboration mutation or agent tool. The release evidence SHALL exercise the real binary, an authenticated hub, at least two independent human principals and local runtimes with separate Git clones, and an unauthorized third principal. It SHALL include messages in both directions, human/agent attribution, held approvals, direct-content isolation, competing claims, disconnect/restart, lost acknowledgement, revocation, repository mismatch and unchanged local permission boundaries. Fixture-based and live issuer/multi-host evidence SHALL be reported separately.

#### Scenario: Complete collaboration acceptance
- **WHEN** Alice and Bob join one project with their locally running agents while Mallory has no membership
- **THEN** Alice/Bob coordinate messages and tasks through CLI/TUI/API/SDK, Mallory receives no project content, and all restart/revocation/isolation checks retain inspectable evidence
