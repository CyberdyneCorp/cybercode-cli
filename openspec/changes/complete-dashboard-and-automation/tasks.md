## 1. Review and baseline

- [x] 1.1 Pull latest main, inspect existing dashboard/collaboration/budget/retention/scheduling contracts, and recheck public live Auth metadata without credentials.
- [x] 1.2 Draft proposal, design, capability deltas and this implementation/acceptance checklist.
- [x] 1.3 Review these artifacts with the user before implementation. On 2026-10-24, after PR #32 and its proposal/design/tasks were presented, the user requested that implementation continue while CyberdyneAuth is unavailable due to payment issues. Continue local implementation; live Auth acceptance remains open. The additional integration queue and native computer-use proposals require their own review.
- [x] 1.4 Record baseline checks and review current main CI: base `78c60566` CI run `40104736761` succeeded; planning validation passes 90 OpenSpec items with 0 failures; canonical lint reports 0 errors/23 warnings. No new implementation tests run at this planning stage; keep unrelated changes out of implementation commits.

## 2. Live CyberdyneAuth acceptance

- [ ] 2.1 Add opt-in live acceptance tooling and provisioning preflight with redacted evidence, no fixture fallback and no secrets in source/logs/arguments.
- [ ] 2.2 Confirm designated test hub, organization and principals; verify/provision public PKCE client, redirects, audience, collaboration scope and least-privilege backend introspection credentials through authorized operator access.
- [ ] 2.3 Exercise actual login, refresh and current organization authorization; test unrelated user, missing scope/audience and inactive credentials against the deployed service.
- [ ] 2.4 Exercise same-org enrollment and external/friends guest admission, including expiry, bound code and replay refusal; verify account MFA remains independent.
- [ ] 2.5 Exercise project membership removal, publication revocation and issuer token/account revocation with designated test resources; record authorization/cache timing and cleanup.
- [ ] 2.6 Fix demonstrated integration defects with regressions; update original hub task 2.4 and live contract only for acceptance actually performed.

## 3. Dashboard collaboration

- [ ] 3.1 Add feature/account/binding-aware collaboration navigation and People view using the local owner API.
- [ ] 3.2 Add project/direct conversations, attribution, threads/history and reconnect/authorization handling without widening visibility.
- [ ] 3.3 Add shared Tasks create/claim/update/report controls with revision and claim fences, queued receipts and visible conflicts.
- [x] 3.4 Add held-message inspection/release/refusal bound to exact project/sender/target IDs and immutable content, reusing receiver policy. Implemented in PR #36 with 30 dashboard unit tests and 23 real-server browser tests; provenance/consent/uncertain retries and inert content covered. Live issuer acceptance remains pending separately in section 2; the full multi-user browser acceptance remains task 3.5.
- [ ] 3.5 Test real hub/server with separate users/agents and two browsers: privacy, stale mutations, lost acknowledgements, consent, revocation and Location switches.

## 4. Reserved budgets

- [ ] 4.1 Add durable reservation/settlement/uncertainty migrations and atomic scope accounting without changing existing soft behavior.
- [ ] 4.2 Integrate request bounds and reservations before all provider dispatch paths, including descendants and auxiliary calls; explicitly reject unpriced/unbounded USD requests.
- [ ] 4.3 Reconcile actual usage once, preserve uncertain post-dispatch allocations across restart, and provide audited owner diagnostics/reconciliation.
- [ ] 4.4 Test concurrent last-dollar admission, nested/daily scopes, unknown pricing, auxiliary calls, duplicate settlement, cancellation and crashes before/after network dispatch.
- [ ] 4.5 Update API/SDK/UI budget evidence, estimates/uncertainty and the original workflow reservation task after verification.

## 5. Finished-agent transcript retention

- [ ] 5.1 Add 30-day workflow-agent retention config and integrate the existing six-hour sweep; zero disables the rule.
- [ ] 5.2 Implement atomic eligibility, pin/replay/live-state protection and tombstones, preserving summaries, costs, solve records and candidate branches.
- [ ] 5.3 Test age boundaries, zero setting, active/reopened runs, reference/pin protection, restart/idempotence, exports and expired transcript UI/API states.
- [ ] 5.4 Update the original workflow transcript-retention task and storage documentation after evidence.

## 6. Durable cron and persistent schedules

- [ ] 6.1 Implement five-field cron/one-shot parsing, pinned IANA timezone, DST semantics and existing per-Session limits/tools.
- [ ] 6.2 Add persistent schedule storage, revision-fenced occurrence claims/admission and server startup/recovery dispatch with bounded skip/run-once catch-up and skip/coalesce overlap.
- [ ] 6.3 Complete `cyber loops` schedule management and matching API/SDK operations; expose next/last outcomes and service availability diagnostics.
- [ ] 6.4 Enforce workspace trust, unattended Mode/bypass policy and all applicable budgets without fabricating interactive presence.
- [ ] 6.5 Complete outstanding loop hooks/notifications, until-goal and exec slash invocation under existing contracts.
- [ ] 6.6 Test fake-clock/DST cases, repeated ticks, pause/edit/remove races, restart during admission, missed runs, unknown execution and unattended permission/budget outcomes with the real server.

## 7. Dashboard settings, attachments and terminal

- [ ] 7.1 Add full schema-backed project/global settings editing, sources, managed locks, validation/change review, secret replacement semantics and restart-required notices.
- [ ] 7.2 Add supported image/PDF uploads and authenticated previews with server limits/modality checks, reference-bound blob reads, inert rendering and resource cleanup.
- [ ] 7.3 Add bundled PTY terminal attachment, input/resize/replay, single-use tickets and capability gating; preserve socket origin checks and never replay uncertain input.
- [ ] 7.4 Test settings scope/secret/policy behavior, invalid/oversized media, malicious content, unauthorized blob reads, expired/cross-job tickets, reconnect/resize and logout cleanup.
- [ ] 7.5 Run keyboard and 360/1280-width real-server browser acceptance for all added surfaces; refresh embedded artifacts and screenshots.
- [ ] 7.6 Complete physical second-device trusted-HTTPS acceptance with the user and record device/browser/hostname/result; update original dashboard task 5.2 only after the actual check.

## 8. Review, CI and documentation

- [ ] 8.1 Update README, feature/roadmap status, configuration/CLI/deployment guides and dated implementation evidence with exact limits and pending external checks.
- [ ] 8.2 Run strict OpenSpec validation/lint, relevant Rust tests/format/Clippy, SDK generation/typecheck/tests, browser typecheck/tests/build consistency and real-server acceptance.
- [ ] 8.3 Perform substantive code/security review using the code-review skill; address findings and achieve green relevant platform CI on reviewable PRs.
- [ ] 8.4 After authorized merge, archive reviewed changes into living specs, reconcile original task lists against merged evidence and verify main CI. Do not mark the overall goal complete while required live/device acceptance is still pending.
