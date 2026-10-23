## 1. Review and implementation boundary

- [x] 1.1 Pull current main, inspect existing Teams/peer/messaging/account/storage contracts and confirm self-hosted hub topology.
- [x] 1.2 Complete and strictly validate proposal, design and capability deltas; synchronize roadmap and document this as proposed work.
- [ ] 1.3 Review these artifacts with the user, including OIDC identity, project membership independent of Git permissions, hub-readable content and local-only execution. Record approval before runtime/auth/data-model implementation.

## 2. Account foundation

- [ ] 2.1 Implement the existing account contracts needed for collaboration: discovery, PKCE login, callback/state/nonce validation, headless completion, ID-token checks, whoami/logout and account-free local behavior.
- [ ] 2.2 Add protected token storage, cross-process serialized refresh/rotation, credential filtering/redaction and collaboration scope/audience checks. Preserve independent LLM-provider login.
- [ ] 2.3 Test real local OIDC protocol fixtures: two subjects, bad issuer/audience/signature, unknown key rotation, expired tokens, missing scope, concurrent refresh, revoked/deactivated users, missing/empty org authorization, stale JWTs after org removal, service-token impersonation, expired authorization leases, Auth outages, step-up authentication, preserved account MFA for internal users and offline local use.
- [ ] 2.4 Document/verify CyberdyneAuth client, audience, required introspection service, optional directory service and scope dependencies (A1/A4/A8). Do not claim live issuer acceptance from a fixture.

## 3. Hub domain, persistence and authorization

- [ ] 3.1 Add typed project/member/runtime/agent/task/message identities, canonical repository descriptors and trusted local bindings; test URL equivalence, forks, credential stripping, remote changes and cross-project refusal.
- [ ] 3.2 Add dedicated hub storage, migrations, durable events/projections, bounded writes and role/visibility-scoped queries. Exercise storage failure, restart, backups and idempotency conflicts.
- [ ] 3.3 Add the separate HTTPS hub listener, OIDC access-token guard, explicit bootstrap principal, scope/audience validation, bounded requests/rate limits and no local-password/peer-token fallback.
- [ ] 3.4 Implement owner/member/viewer roles, organization/friends modes, pinned org IDs, internal invite-only/opt-in org enrollment without an extra code, policy-gated external guest grants, recipient-bound invitations or owner-approved pending applications, last-owner/step-up guards, policy restrictions and audited role changes/removal. Include outsider and cross-project negative cases.

- [ ] 3.5 Add secret guest/friends approval codes: identity/project/role binding, ten-minute expiry, five attempts, issuance limits, keyed verification, atomic one-time activation, reissue/replay/crash tests, and no bypass of personal Auth MFA or internal-only trust rules.

## 4. Published participants and messaging

- [ ] 4.1 Explicit owner-driven publish/unpublish, agent capability issuance/renewal/revocation, repository-bound Sessions and presence; never expose private local state or raw credentials.
- [ ] 4.2 Human/project/direct conversations with authoritative human/agent attribution, threading, Git references, scoped visibility and bounded content. Test impersonation and indirect disclosure through history/events/snapshots.
- [ ] 4.3 Local agent tools and inbox integration through existing permission/hook/Mode/budget boundaries. Hold by default, preserve non-interactive refusal, and reauthorize delivery/release after membership or policy changes.
- [ ] 4.4 Durable outbox/inbox acknowledgements, seven-day expiry, cursor replay/resync, reconnect backoff and visibility-filtered streams; test lost acknowledgements and crashes without duplicate admission.

## 5. Shared task coordination

- [ ] 5.1 Task creation/dependencies/comments/results, expected revisions, atomic claim generations and role-bound updates. Run concurrent-claim and stale-update tests.
- [ ] 5.2 Preserve offline/revoked assignees as explicit unresolved coordination state; implement owner-reviewed reconciliation without automatic execution failover or a false native-settlement claim.
- [ ] 5.3 Retain separate local worktrees and attributed Git/test references; refuse remote evidence that attempts to manufacture local solve authority or automatic merges.

## 6. Product surfaces and documentation

- [ ] 6.1 Add the reserved `collab` CLI command tree, account/setup errors, project join/bind/member/publish/message/task flows and diagnostic status.
- [ ] 6.2 Add versioned hub/local OpenAPI operations and generated SDK groups; test the real service with separate principal/agent credentials, not only mocked response shapes.
- [ ] 6.3 Add the TUI `/collab` participants/conversation/task views and integrate attributed held-message cards, offline/revoked state and permission requests owned by the receiving user.
- [ ] 6.4 Update README, feature/usage/deployment guides, roadmap, security/retention/backup limits and the canonical contract at archive. Clearly separate this feature from direct peers, local Teams, remote execution and the Relay.

## 7. Acceptance, review and delivery

- [ ] 7.1 Real-process acceptance with a hub, local OIDC fixture, Alice and Bob runtimes/clones and unauthorized Mallory: people and agents communicate both directions and claim/update shared work through actual clients.
- [ ] 7.2 Fault/authorization matrix: dropped acknowledgements, competing claims, hub/client restarts, offline replay/expiry/resync, hub membership/agent revocation, upstream user/org removal with a maximum 60-second window, no stacked cache extension, pending-message release, cross-project access, spoofed authorship and unchanged local permissions/checkout state.
- [ ] 7.3 Run platform-appropriate integration suites, all-target Clippy, formatting, SDK/type generation and strict OpenSpec checks; perform code review using the code-review skill and fix findings.
- [ ] 7.4 Retain exact commands/results, test environments and remaining live-issuer/multi-host evidence. Do not call unsupported acceptance types passed.
- [ ] 7.5 Deliver descriptive, reviewable incremental PRs with green CI. After implementation merge and evidence review, archive the change into living specs and verify the resulting main CI.

## Proposal validation (2026-10-23)

Pinned OpenSpec 1.13.2 strict validation passes all 86 items. Canonical specification lint reports 0 errors and 23 warnings. The canonical inventory remains 49 capabilities / 899 requirements because this is an unimplemented proposed change. Changed Markdown relative file links and whitespace checks pass. Rust/SDK runtime tests were not rerun for this specification-only increment; implementation and its acceptance remain unchecked above. Topology selection is recorded; artifact approval (1.3) is still pending.
