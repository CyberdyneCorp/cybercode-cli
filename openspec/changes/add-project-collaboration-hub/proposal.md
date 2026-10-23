## Why

An organization or group of friends should be able to work on one Git project with each person's coding agents participating alongside them. Today, local Teams coordinate one owner's Sessions, while direct peers deliberately connect machines belonging to one person. Neither supplies cross-person project membership, a shared conversation and task board, or durable delivery to another person's local agent.

The user selected a **self-hosted project hub**: one reachable server coordinates people, messages and tasks, while every member's agents, credentials, tools and checkouts remain on that member's machine.

## What Changes

- Add an experimental P3 `project-collaboration` capability and `cyber collab` commands. One hub hosts multiple isolated projects, each anchored to a canonical Git repository identity; GitHub is the first documented example.
- Give each person a currently active authenticated identity and explicit project membership. Organization projects pin a CyberdyneAuth organization ID: verified internal members need no extra project join code when the project join policy allows them. External guests require an explicitly enabled, owner-approved guest grant plus a user/project-bound verification code; friends' projects use the same authenticated invitation/code path. Owners grant owner/member/viewer project roles. A repository URL, matching clone, GitHub organization name or agent-supplied author field never grants access.
- Let humans and explicitly published local agent Sessions exchange attributed project messages and participate in project conversations. Incoming agent prompts are held for the receiving user's approval by default.
- Provide one durable project task board: owners, dependencies, atomic claims, revisions, comments and Git commit/PR references. A disconnected assignee keeps its claim; a timeout never authorizes another machine to repeat its work.
- Persist offline delivery, idempotent operations and replay cursors. Reconnect, revocation and crash recovery must preserve project boundaries and avoid duplicate inbox admission.
- Expose the same operations through the local API, hub API, CLI, TUI and TypeScript SDK. An agent's project membership never widens its local permissions or spends another person's inference budget.

## Scope and Delivery

This change owns the complete usable self-hosted collaboration slice, including the account foundation it needs. It is **proposed, not implemented**. Runtime implementation starts only after review of this proposal, design, scenarios and tasks.

1. Account foundation: discovery-based CyberdyneAuth/generic OIDC login, secure storage and serialized refresh, validated access-token identity and collaboration scope. Preserve account-free local use.
2. Hub foundation: isolated storage/listener, repository identities, memberships, invitations, revocation, participant registration and project authorization.
3. Collaboration: human/agent messaging, safe local inbox integration, persisted delivery/replay and shared task coordination.
4. Product surfaces and acceptance: CLI/API/SDK/TUI, two independent users and runtimes, outsider and cross-project refusal, competing claims, restarts, offline delivery and removal of a member.

Each increment needs its own regression coverage and reviewable PR. Scaffolded commands or an in-memory demo do not complete the goal. Final acceptance must exercise the real binary, independent runtimes, an authenticated hub and separate clones of the same Git repository.

## Decisions for Review

- **Identity:** reuse the repository's CyberdyneAuth/OIDC direction, rather than treating a friend's machine token as a human account. A self-hosted issuer is supported. Issuer/client registration and the new scope are deployment dependencies; an unverified GitHub handle is only a display label.
- **Membership:** project creation explicitly selects an organization-bound project or a friends' project. Internal members must be currently authorized by Auth for the pinned org and permitted by the project's join policy; they receive no additional project OTP challenge. Same-org access does not automatically open every private project. External users require an active Auth account, owner-approved guest invitation and an expiring user/project-bound join code. Guests are disabled by default for organization projects and may be forbidden by org/deployment policy. A project approval code is distinct from account MFA: existing CyberdyneAuth SSO/MFA is never disabled or shared. GitHub remains the Git host; Auth org labels are not independent proof of GitHub repository permissions.
- **Hub trust:** this hub is an authorized reader of shared project messages and tasks. TLS protects transport. It is not the future ciphertext-only remote-control Relay, and no end-to-end encryption claim is made for hub-held content.
- **Execution:** members explicitly publish Sessions and decide locally whether to accept incoming work. The hub cannot run shell commands, approve another person's tools, copy private transcripts, merge branches or move a Session between machines.
- **Initial deployment:** a single-instance self-hosted hub uses its own SQLite database, following the accepted storage decision. Hosted multi-instance PostgreSQL and a managed SaaS deployment remain separate work.

## Capabilities and Contract Alignment

- ADDED `project-collaboration` (P3): repository-bound membership, human/agent attribution, shared messages/tasks, recovery, privacy and client surfaces.
- MODIFIED `cyber-account`: request the collaboration scope and include the hub among access-token validating services.
- MODIFIED `cross-session-messaging`: distinguish project-hub messages from existing own-machine peers and the future account Relay.
- ADDED requirements in `server-api` and `agent-teams`: separate hub authorization/listener and human-owned project participation without changing local Team ownership.
- MODIFIED `cli-commands`: reserve `collab` in the authoritative command registry.

## Non-Goals

Managed cloud hosting/billing; implementing all remaining P3 features; GitHub App/webhook automation or automatic synchronization of GitHub permissions; arbitrary remote execution; automatic patch application or merging; sharing complete Session histories, provider keys or files; automatic task failover after loss of contact; a new browser dashboard; changing the existing direct-peer trust model.

The local web dashboard may later consume these APIs, but is not a prerequisite. Existing P2 Teams, verified workflows and peer messaging must keep working without a Cyber Account.

## Status

2026-10-23: pulled `main` at `8e7085a02dec908713ccdbaf96542e5d7826dbf3`; GitHub had no open PRs. The new implementation goal is active. Self-hosted hub topology was selected by the user. This proposal is awaiting artifact review; it is not implementation approval.

Security review clarification: use CyberdyneAuth live token introspection to distinguish active user accounts from service credentials and refresh the authorized organization set. Organization/project authorization is separate from Git hosting access. The [live Auth public contract](auth-contract.md) has been checked; authenticated flows, required provisioning and collaboration implementation remain unverified.
