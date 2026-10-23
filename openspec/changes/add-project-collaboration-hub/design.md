## Context

The binary is server-first: local clients use one per-user runtime, SQLite events and projections are durable before acknowledgement, and unknown external effects are not replay authority. Existing direct peers use machine-scoped grants, hold inbound prompts by default and intentionally exclude different people. P2 Teams assume one lead's ownership, Mode ceiling and budgets. Neither boundary can safely become multi-user authorization just by accepting `name@machine`.

The selected topology is a self-hosted collaboration hub with outbound connections from members' local servers. Friends need access to the hub, not inbound access to one another's laptops. One person's local server password never crosses that boundary.

## 1. Domain and Identity

Keep these identities separate:

| Identity | Authority |
|---|---|
| Account `(issuer, sub)` | Validated OIDC identity of a person; names and email are display fields |
| Collaboration project `cprj_…` | Hub-issued resource, with explicit membership and a canonical repository descriptor |
| Member `cmem_…` | Account's role in exactly one collaboration project |
| Runtime `crt_…` | One member's enrolled local server instance/device |
| Published agent `cagt_…` | Runtime-owned, opted-in Session in the project's bound Git checkout |
| Task `ctask_…`, claim generation | Shared coordination state, not ownership of another machine's execution |

Do not reuse the local `prj_` hash as authentication or authorization. The current local Git identity is useful for locating checkouts, but its shortened digest and remote normalization are not a network security boundary.

The hub assigns author and project identities from authenticated membership or a scoped agent capability. Requests cannot choose a different member, attribute an agent message to a human, or impersonate another member's agent. The member controls its own registered runtime and Sessions; the hub does not attest which model actually generated their text.

## 2. Account Foundation and Deployment

Implement the `cyber-account` contracts required for this slice: discovery, public authorization-code/PKCE login, loopback or headless-paste completion, ID-token checks, account display/logout, protected keyring/file storage and cross-process refresh rotation. Add `cyber:collaborate` to requested scopes; a missing scope disables collaboration with an actionable error, never a less authenticated fallback.

Hub user access tokens require the configured issuer, valid signature/time, a nonempty subject, a mandatory configured API audience and `cyber:collaborate`. An ID token, service account, LLM subscription credential, local password or peer grant is not a human identity. JWT/key handling uses supported libraries and issuer discovery; refresh, issuer changes, key rotation and revocation get contract tests. For this CyberdyneAuth integration, server-to-server introspection is required: the result must be `active: true`, `token_type: user`, match the validated subject/audience and retain the required scope. The backend service credential used for introspection is distinct from the user's token and never reaches clients or models. An issuer without a supported live-validation contract cannot silently fall back to signature-only collaboration authorization.

CyberdyneAuth's current introspection implementation rejects disabled/deleted users, revoked sessions/tokens and disabled OAuth clients, and recomputes `orgs` from active organization memberships. Pin organizations by their immutable ID from that current authorized set, not email domain, primary `org`, display name or a request field. The organization-members directory can support an owner user picker with a separate server-side `directory:read` credential; listing it is not necessary to trust a caller-supplied organization claim. An Auth organization and its admin-managed `github_login` are not live GitHub membership checks.

Join, invitation redemption/approval, publication, role/owner changes and project-policy changes require uncached live user/organization validation. Ordinary project requests and agent authorization leases may reuse a positive result for at most 60 seconds, never beyond the access token's expiry. Derived runtime leases/capabilities must not extend this deadline: use the earliest underlying identity, organization and lease deadline rather than stacking cache lifetimes. Unavailable/unknown validation cannot refresh a lease or fall back to stale JWT claims; after the bounded authorization window it fails closed. Hub-local project/agent revocation is checked in each operation/transaction. Upstream user deactivation or removal of required organization membership has a documented maximum 60-second authorization window; it is not falsely described as instantaneous cancellation of local OS work.

The deployment operator configures the issuer, audience and initial hub administrator `(issuer, sub)` through trusted local configuration. First login does not claim the hub. Local project files cannot change these values, open a listener, add a hub or publish an agent. A single-instance self-hosted deployment may disable commercial entitlement checks; it still requires identity, scope and project authorization. This is not permission to bypass account validation.

Upstream dependencies are explicit: register the public CLI client and access-token audience, register the required backend introspection service (and optionally scoped org-directory access), and add the collaboration scope in CyberdyneAuth (ROADMAP A1/A4/new A8). Tests use a real local OIDC protocol fixture with separate signed principals and live introspection state. Live issuer acceptance must be reported separately from fixture results; no deployment configuration is silently assumed to exist.

Read-only upstream source review confirms an admin-managed scope/resource registry and optional device authorization support. This makes registering a collaboration scope/client a provisioning dependency, not evidence that an issuer source change is necessarily needed. Device login is selected only when discovery advertises it and the client is permitted to use it; otherwise keep the documented PKCE/headless fallback. No running issuer was configured or tested during this proposal.

## 3. Repository Binding and Membership

A project descriptor contains its hub ID, name and canonical Git repository identity. For GitHub, SSH and HTTPS forms of the same host/owner/repository normalize together with credentials removed. Preserve host/port distinctions, distinguish forks, reject local filesystem remotes for network identity, and do not publish credentials or local absolute paths. An explicit owner-reviewed migration handles a repository rename/transfer; a changed `origin` never silently rebinds a running Session.

Repository identity locates a project; it proves neither Git hosting access nor collaboration membership. A user joins through an owner-issued, single-use, 256-bit invitation with a default 24-hour expiry. Store only its hash. Prefer binding the invitation to an expected `(issuer, sub)` selected from verified identity data. If the recipient is not known at issue time, redemption creates a pending application containing the authenticated identity; an owner must approve it before any project history, participant data or write capability is disclosed. Recheck all identity/project policy checks at approval. Possession of an unbound invitation alone never grants access. Joining requires interactive confirmation of hub, repository and role and does not grant GitHub repository access. A clone or `.cyber/` file can suggest a descriptor but cannot accept membership or send data.

Each project explicitly selects one membership mode at creation:

| Mode/participant | Required authorization | Extra project join code |
|---|---|---|
| Organization / internal | Active Auth user, current membership in pinned `organization_id`, and permitted project enrollment/role | None |
| Organization / external guest | Guests explicitly allowed by project and org/deployment policy, active Auth user, owner-approved identity-bound guest grant | Required |
| Friends | Active Auth user and owner-approved identity-bound invitation/application | Required |

Organization projects default to `invite_only`: same-org users still need a project role/invitation/approval, but no extra project code. An owner may explicitly choose `organization_members` enrollment with a default viewer role (or member when explicitly selected); it can never auto-grant owner. Thus an organization can deliberately let its staff join easily without making every private project visible merely because a person has an org claim.

Organization projects default to external guests disabled. Enabling them requires both trusted org/deployment policy and explicit project-owner consent; the owner cannot bypass a deployment prohibition. Guest grants are a separate membership kind, visibly labelled in the roster, messages and published agents, default to viewer, may explicitly permit member, and never confer owner or org membership. Turning guest access off invalidates those grants and pending challenges. A removed internal org member is not automatically converted into a guest: new external access requires a new approved grant and code. Internal-only delivery allow rules do not silently match guest agents.

The mode and organization binding remain immutable. Missing/empty `orgs` never means all organizations; an unrelated org role or global `is_admin` flag cannot substitute for the required project role. Org member administration remains in CyberdyneAuth. Project owners cannot add people to the Auth org, and the hub does not assume Auth org membership proves GitHub permissions.

### Guest/friends join code

After the owner approves the authenticated recipient and allowed role, the hub issues an eight-digit cryptographically random join code, expiring after ten minutes. Bind it to the opaque challenge, project, invitation/grant, expected `(issuer, sub)` and allowed role. Permit at most five attempts, one active challenge per user/project invitation and three issuances per user/project per hour. Reissue invalidates the prior challenge. Store a keyed verification MAC with a server-held key outside the database, not plaintext or a bare hash of a small numeric code; never log or expose it to model tools. Redeem and consume it atomically with fresh Auth/policy validation and membership activation. A retry after successful admission uses ordinary operation idempotency and cannot reactivate revoked membership.

The founding owner of a new friends project is established through authorized hub project creation and fresh authentication, not a circular self-invitation. Subsequent invitation-based membership follows the code flow.

The code is a one-time join/rejoin check, not a challenge for every message or ordinary reconnect of an active member. Revoked membership needs a new approved invitation/code rather than reuse of an old proof.

The owner conveys this separate project code to the intended person through their chosen trusted channel. It is an application approval step, not a claim of a second authentication factor. Do not ask for, forward or share the owner's personal authenticator code. Account MFA remains an Auth responsibility and mandatory managed MFA still applies, including when no extra project code is needed internally. For an external/friends recipient, a bound invitation or approved application alone remains pending until the code is consumed; no project history/capability is exposed beforehand.

Project roles:

| Role | Shared state rights |
|---|---|
| Owner | Manage project metadata, invitations, roles and revocation; reconcile task ownership; all member rights |
| Member | Read project content, post messages, publish owned Sessions, create tasks, claim unowned tasks and update permitted owned work |
| Viewer | Read authorized project content; cannot send, publish, claim or mutate |

The last owner cannot be removed/demoted. Invitation/membership administration requires an authentication time within 15 minutes (and managed MFA when required); refreshing a token does not renew that authentication time. Role changes, invitation use and revocation are audited. Friends without the pinned org claim can join a friends-mode project, or an explicitly permitted external guest grant, only through the approval/code path. Inaccessible project IDs return no project metadata. The initial limits are 100 members, 1,000 published agents and 10,000 open tasks per project; limits are explicit and bounded in trusted hub configuration.

## 4. Local Runtime and Published Agents

After login and explicit project join/binding, the local runtime connects outward to the configured HTTPS hub. Presence heartbeats are every 20 seconds; a participant becomes offline after 60 seconds without one. Presence is observation, not permission or execution settlement.

Publishing requires the Session's owner to opt in and bind it to a confirmed project checkout. Publish only its agent ID, member, alias, observed state and explicitly selected Git references. Do not upload the Session's transcript, environment, provider key, prompts, local path or full tool output. Return a short-lived, project/agent-scoped capability to the local runtime, never to the model. The hub retains only its hash. It permits message/task/presence operations allowed to that agent, not member administration, human impersonation or access to another project; renewal requires a still-active membership. Unpublishing or membership removal invalidates it and closes its subscriptions.

Capabilities expire after 15 minutes and require an active runtime authorization lease bounded by their owner's latest live user/org validation, at most 60 seconds old. They cannot survive loss of the owner's required org membership by relying on their longer token lifetime. Local logout stops renewal and attempts immediate capability revocation; hub-side membership revocation is enforced on every request. Credential storage/redaction must cover these capabilities as well as account tokens. A Session's published project identity is immutable: unpublishing does not authorize moving its private accumulated context to another project. A different project uses a separate Session.

Integrate through focused collaboration domain/store/service modules and a local runtime adapter. Keep coordination state separate from the Session Drain. Do not turn the local `AppState` owner password into multi-user authentication, and do not make the hub a model/tool runtime.

## 5. Messages, Consent and Delivery

Provide a project conversation and addressed messages to a member or published agent, with thread/reply IDs and optional Git commit/PR references. Each record distinguishes human versus agent authorship, owner, target, project, content hash and server-assigned timestamps. Text is at most 16 KiB; there are no file attachments in this slice. Human CLI/TUI messages require no inference.

Project conversation is visible to active project readers. Direct messages are visible only to participants and the human owners of participating agents; an unrelated project owner has no API right to read their content. The deployment operator remains a trusted storage administrator, which is disclosed rather than presented as end-to-end privacy. Apply the same visibility checks to history, search, snapshots and streamed events.

Reading or posting a project conversation does not automatically prompt all published agents. Only an explicitly addressed agent, or an owner-enabled subscription, can receive a local inbox admission, still governed by consent and rate/chain limits.

Agent messages use ordinary local send permissions and existing size, recursion and rate protections. Incoming project text is untrusted data. Agent-directed messages enter the target Session's inbox held by default; only an explicit project/member/agent rule or the receiving user's release permits prompt admission. Existing `dont-ask`, non-interactive inbox refusal, workspace trust, Mode ceilings, deny rules, hooks and budget boundaries remain effective. A project owner cannot approve another member's local permission requests or change their model/configuration.

The hub durably accepts an outgoing message before returning `queued`. Offline agent delivery is retained for seven days, then becomes `expired`. A local connector commits the target inbox record before acknowledging delivery. Stable keys `(hub, project, message, target)` deduplicate retries after either side crashes; the same key with different content is a conflict. Report queued, held, released, refused, dropped and expired accurately; neither acceptance nor delivery claims the recipient read or executed the text.

Recheck membership, publication and inbound policy before delivery and again before releasing a held prompt. Revoked senders cannot have undelivered work released later. Already-admitted text remains historical untrusted input; revocation does not pretend to cancel an already running OS command. There is no automatic reply loop and no automatic remote agent launch.

## 6. Shared Tasks

Tasks contain title, description, status, assignee, dependencies, revision, claim generation, timestamps and optional Git references/results. Mutations require an expected revision. The hub performs membership authorization, dependency checks, atomic claim/update, event append and projection in one short transaction. Competing claims have one winner. An agent can claim only for itself and cannot reassign another member's work; owners can request reconciliation/reassignment.

Claims survive disconnects and hub restarts. An offline or removed assignee is visible and blocked for coordination; no lease timeout silently returns its work to `todo`. Reassignment must explicitly address the prior ownership/unknown-work state. Stale claim generations cannot update the new assignment. A task claim never proves that native work was stopped or grants another actor permission to duplicate an external effect.

Git continues to distribute code. Members work in their own clones/worktrees and share commit/branch/PR references. Marking a task done is an attributed report, not a host-verified test result. A solve result from another runtime is not allowed to manufacture this runtime's verification badge. Applying or merging a patch remains a local, explicit action.

## 7. Storage, Streaming and Recovery

Use a dedicated single-instance hub SQLite store, WAL, synchronous FULL and the existing bounded-writer/event-projection pattern. Hub API acknowledgement follows commit; storage failure stops mutations. Do not replicate SQLite files or place them on network filesystems. PostgreSQL/multi-instance hosting follows the accepted storage architecture as later work.

Every mutation has a stable operation ID scoped to principal/project and a canonical payload hash. Matching retries return the original result; a mismatch is 409. Events have monotonic project cursors; subscriptions recheck authorization and emit only content the principal may read. Reconnect uses cancellable 1–60 second backoff. Retain replay events for 30 days; an expired cursor returns a typed resync-required response and the client rebuilds from an authorized snapshot. A gap never triggers an execution replay. Hub and client restarts, lost acknowledgements and simultaneous claims have real-process acceptance cases.

Recheck membership/role inside the transaction that mutates shared state or returns an idempotent result; a concurrent removal must not race a previously authorized mutation into committing afterward. Keep network I/O and model work outside those transactions.

Keep message payloads, membership/task state and security audit entries subject to documented retention/backups. Credentials and invocation tokens never enter events, exports, prompts or Git. Polling/streams are bounded and rate-limited per member as well as per project; multiple agent IDs cannot bypass the member limit.

API defaults are 64 KiB request bodies, 128 active connections, 120 requests/minute per member and 3,000 per project. Batch agent presence through the owning runtime so heartbeats need not create one connection/request per agent. A rate-limited connector honors the retry delay and preserves queued operations and their IDs.

## 8. Public Surfaces

Proposed commands (not available yet):

```text
cyber collab serve --issuer <url> --audience <aud> --bootstrap-sub <sub>
cyber collab project create --repo <git-url> --name <name> --organization <org-id>
cyber collab project create --repo <git-url> --name <name> --friends
cyber collab invite --project <id> --role member [--subject <sub>] [--guest]
cyber collab invitations list|revoke --project <id>
cyber collab join [--invite-stdin]
cyber collab applications list|approve|reject --project <id>
cyber collab bind --project <id> [--remote origin]
cyber collab members list|set-role|remove --project <id>
cyber collab publish --project <id> --session <session> --name <alias>
cyber collab unpublish <agent>
cyber collab messages list|send --project <id> ...
cyber collab tasks list|create|claim|update|reconcile --project <id> ...
cyber collab leave --project <id>
```

The hub binds loopback by default (port 4749). Off-loopback clients require HTTPS; support operator-managed TLS or a documented HTTPS reverse proxy, with no certificate-verification bypass. Serve only hub coordination routes. Local management/Session integration stays on the per-user API. Distinguish both in OpenAPI with listener metadata and generate typed SDK groups. Bound every route by authenticated project identity, not `x-cyber-directory`.

Joining accepts an interactive secret paste or stdin rather than requiring an invitation in shell history. Invitation/challenge issuance prints its one-time secret only to the requesting human; diagnostics, logs and normal member listings never print it. Joining prompts for any required project code as a secret, never a personal Auth MFA code.

The TUI `/collab` panel shows the bound project, people/agents with offline status, conversation and tasks. Messages clearly name human/agent authorship. Existing held-message cards handle local consent. Model tools list participants, read authorized conversation/thread history, send messages and read/create/claim/update/comment on tasks only for the current published Session/project. Returned messages and task text remain untrusted tool data. Agent sends use `collab.message.send` and task mutations use `collab.task.write`, both asking by default unless explicitly allowed; project publication does not silently allow mutations. No invite, bind, role-management or publishing tools are offered to agents.

Expose the feature behind `features.collaboration`, off by default until acceptance. Feature-off exposes no collaboration tools and admits no collaboration mutation. Disabling it does not affect existing local Teams or own-machine peers. CLI errors distinguish unauthenticated/missing scope, forbidden role, membership revoked, repository mismatch, revision/idempotency conflict, offline queued delivery and expired replay.

## Risks and Open Decisions

- OIDC client/service registration and the new scope need upstream/operator configuration. Implement fixture-tested behavior without claiming live deployment acceptance until it has run.
- Identity proves the account and its Auth memberships, not employment, benign agent behavior or GitHub access. Organization administrators remain responsible for correct onboarding/offboarding; cached authorization has the bounded window described above.
- Hub administrators can read shared state. This is an explicit trusted coordination service; ciphertext-only Relay remote control remains unchanged.
- Git hosting permissions and collaboration memberships are independent. Automatic GitHub membership synchronization would require a separate reviewed integration.
- Browser UX, managed SaaS, remote Runner scheduling and automatic Git bundle transfer are excluded. The first complete experience is CLI/TUI/API/SDK on a reachable self-hosted hub.
- The user has selected hub topology. Approval of this design, including account-based identity and trusted-hub content storage, remains pending.
