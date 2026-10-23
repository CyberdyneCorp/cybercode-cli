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
(P3) Membership SHALL bind validated `(issuer, sub)` identity to exactly one project role: owner, member or viewer. Owners SHALL manage project metadata, roles, invitations and revocation; members SHALL read, message, publish owned agents and coordinate permitted tasks; viewers SHALL be read-only. A single-use invitation SHALL use 32 random bytes, default to 24-hour expiry, store only its hash and be consumed atomically. It SHALL either bind to an expected `(issuer, sub)` or create a pending authenticated application that an owner must approve before any project data/capability is disclosed. Approval SHALL revalidate current identity and project policy; possessing an unbound invitation SHALL NOT automatically activate membership. Only owners SHALL authorize membership/role changes, including an explicit organization-enrollment policy; the hub SHALL NOT grant beyond that policy, and the last owner SHALL NOT be removed or demoted. Invitation/membership administration SHALL require authentication within the last 15 minutes and any managed MFA requirement; token refresh SHALL NOT count as fresh authentication. People without the pinned organization membership SHALL require a friends-mode invitation or an explicitly permitted external-guest grant, including the required join code; org claims and Git hosting access SHALL NOT automatically enroll anyone. Defaults SHALL bound each project to 100 members, 1,000 published agents and 10,000 open tasks.

#### Scenario: Invitation reuse
- **WHEN** two accounts concurrently redeem the same invitation
- **THEN** at most one matching redemption/application is committed, activation still requires recipient binding or owner approval, and the other receives no project content or reusable credential

#### Scenario: Stolen invitation
- **WHEN** a different authenticated account obtains an invitation intended for another person
- **THEN** a subject-bound invitation refuses it, while an unbound invitation grants only a pending application with no project access until owner approval

#### Scenario: Viewer tries to claim work
- **WHEN** a viewer sends a message, publishes an agent or mutates a task
- **THEN** authorization refuses the operation without a durable mutation

#### Scenario: Membership does not grant Git access
- **WHEN** a friend joins a project without access to its private GitHub repository
- **THEN** collaboration membership does not create GitHub access, and the client explains that Git permissions are managed separately

### Requirement: Explicit organization and friends project modes
(P3) Project creation SHALL explicitly choose `organization` mode with a pinned immutable Auth `organization_id`, or `friends` mode. Internal organization access SHALL require an active user, current membership in that exact Auth organization and a permitted project enrollment/role; every project owner SHALL satisfy that organization check. Internal users SHALL NOT face an extra project join-code challenge, while CyberdyneAuth SSO/MFA policy SHALL remain effective. Organization projects SHALL default to `invite_only`; an owner MAY explicitly permit `organization_members` enrollment with default viewer or explicitly selected member role, never owner. The stored mode/org binding SHALL remain authoritative. External guests SHALL default to disabled and require both org/deployment policy permission and explicit project consent. Each guest SHALL have a separately labelled, owner-approved grant and pass the user-bound join-code flow; guest role SHALL default to viewer and SHALL NOT exceed member. Disabling guests SHALL revoke their grants and pending challenges. Removed internal members SHALL NOT automatically become guests. Friends mode SHALL require active authenticated users and the approved invitation/code flow. Email/domain, a URL, a global admin flag or an unrelated role SHALL NOT substitute for authorization, and Auth org membership SHALL NOT be presented as a live GitHub permission check.

#### Scenario: Valid account outside the organization
- **WHEN** an active authenticated user holds an invitation but is outside the pinned Auth organization and external guest access is disabled
- **THEN** the project refuses enrollment and access without disclosing content

#### Scenario: Same organization without a project role
- **WHEN** an active member of the pinned organization has no project membership and the project is invite-only
- **THEN** the organization claim alone does not expose the project or its agents

#### Scenario: Same organization without an extra code
- **WHEN** current Auth validation proves the user belongs to the pinned organization and its project enrollment/role is allowed
- **THEN** it can join without an additional project OTP, while existing Auth SSO/MFA requirements still apply

#### Scenario: Approved external guest
- **WHEN** both deployment/project policy permit guests and an owner approves an active external user's guest grant
- **THEN** the user remains pending until the bound join code is consumed, then receives only the approved guest role and visible external attribution

#### Scenario: User belongs to multiple organizations
- **WHEN** the primary `org` differs but current authorized `orgs` contains the project's exact organization ID
- **THEN** the organization gate may pass, while the separate project role is still required

#### Scenario: Owner attempts to bypass the org gate
- **WHEN** an owner changes the stored org identity, hides guest status or enables guests against deployment policy
- **THEN** the stored binding, guest attribution and policy remain authoritative and the attempted bypass is refused

### Requirement: Bound guest approval codes
(P3) Invitation-based external-guest and friends-mode activation SHALL require an owner-approved authenticated recipient and a separate eight-digit cryptographically random project join code. Bind the code to project, invitation/grant, expected `(issuer, sub)` and role; expire it after ten minutes, allow at most five attempts and one active challenge per recipient/project invitation, and cap issuance at three per recipient/project per hour. Reissue SHALL invalidate the old challenge. Store only a keyed verification MAC under a server-held key, never plaintext or an unkeyed hash of the small code space. Fresh identity/policy checks, attempt accounting, one-time code consumption and membership activation SHALL be atomic. No project history or agent capability SHALL be granted before success. Codes SHALL be absent from logs, model tools, URLs and normal listings. This flow SHALL NOT disable account MFA or request another person's personal authenticator code. It SHALL apply to invitation-based joining/rejoining, not every message or ordinary reconnect of an already authorized member. Founding project-owner creation SHALL use authorized hub creation and fresh authentication rather than a circular self-invitation.

#### Scenario: Code used by the wrong user or project
- **WHEN** a valid code is submitted under a different subject, project, invitation or role
- **THEN** activation fails and no authority is transferred

#### Scenario: Expired, guessed or replayed code
- **WHEN** a challenge expired, exhausted five attempts, was replaced or was already consumed
- **THEN** it cannot activate membership, including under concurrent requests or after a server restart

#### Scenario: Authorized guest reconnects
- **WHEN** an already admitted guest reconnects with active Auth identity and a still-authorized guest grant
- **THEN** normal authorization is revalidated without requiring a new project code; a revoked grant instead requires a new approved invitation and code

#### Scenario: Guest message does not inherit internal trust
- **WHEN** a new guest agent messages a Session that only auto-delivers verified internal members
- **THEN** the message remains held/refused unless an explicit guest-authorizing local rule permits it

### Requirement: Fresh active-user and organization validation
(P3) The CyberdyneAuth integration SHALL combine signature/issuer/audience/expiry/scope checks with authenticated server-to-server introspection that reports `active: true`, `token_type: user` and the matching subject. Internal organization checks SHALL use the current authorized `orgs` returned by that live validation, never only an old JWT snapshot; missing/unknown/empty organization authorization SHALL NOT mean all organizations. Join/redemption/approval, publication and owner membership/policy mutations SHALL use uncached validation. Ordinary access and delegated agent/runtime leases SHALL depend on positive identity/org validation no older than 60 seconds, bounded by token expiry; nested caches/capability lifetimes SHALL NOT extend that deadline. Upstream user deactivation or removal of required organization membership SHALL deny new authorization within this maximum window, while local project revocation SHALL be checked on every operation. An outage/unknown response SHALL NOT renew authorization or trigger a signature-only fallback. An issuer without a supported live validation contract SHALL be refused for this feature. Service credentials SHALL remain backend-only and SHALL NOT enroll as people.

#### Scenario: Stale JWT after organization removal
- **WHEN** a user's signed JWT still lists an organization but live introspection no longer authorizes it
- **THEN** the internal membership and its agents are refused despite the unexpired JWT and stored project role, with no automatic conversion to external guest

#### Scenario: Deactivated user in a friends project
- **WHEN** introspection reports the previously invited account inactive
- **THEN** friends-mode membership does not authorize new reads, messages, claims or agent capability renewal

#### Scenario: Stale cache during Auth outage
- **WHEN** Auth cannot validate a user and the last positive validation has reached its 60-second deadline
- **THEN** requests, stream delivery, held-message release and delegated-agent authorization fail closed without extending a runtime lease

#### Scenario: Service token presented as a person
- **WHEN** a valid service/client-credentials token is used to join or claim human membership
- **THEN** it is refused as a human identity even if it has a valid signature and collaboration-looking scope

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
