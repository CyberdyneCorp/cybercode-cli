# Design

## Current implementation and review boundary

The browser is a bundled TypeScript client in `web/`, embedded by `cyber-server`. It already uses explicit owner authentication held in page memory, fetch-based SSE, Session attachment leases and stale-request guards. Collaboration has local CLI/TUI/API/SDK surfaces and a dedicated hub. PTY Jobs already expose an owner-authenticated ticket endpoint and a single-use WebSocket connection endpoint. The App retention worker runs after 120 seconds and every six hours. Session/workflow budget validation currently rejects `reserved` rather than silently using soft enforcement.

This design preserves those boundaries. Public API additions require OpenAPI/SDK updates; browser state, rendering and transport remain separate. New browser functions should stay within the project's complexity guidance. Artifacts describe the proposed implementation; unchecked tasks are not claims of shipped behavior.

## 1. Live identity acceptance

Use the configured issuer and discovered endpoints. The current reference deployment is `https://auth.backend.coolify.cyberdynecorp.ai`; public discovery and OpenAPI were fetched read-only on 2026-10-24. SHA-256 values remain `293ddaac98f8787411fcb35d4b7a7a4168613a9f607db0b3dad290e20e7b645f` and `0a76e139c858e440a417dfdf01f1e62ed72c732c5bd189c6a20ae4e1ea85bdb3`. Device authorization is not advertised and the collaboration scope is absent. Do not enable an unadvertised grant or drop a scope check to make acceptance pass.

Prepare an opt-in live acceptance harness and operator checklist. Keep credential files outside the checkout with restrictive permissions; reports contain only redacted assertions and non-secret resource identifiers. Run ordinary CI against isolated protocol fixtures, never against production accounts or real organization mutations. Live tests must explicitly select their issuer, test hub and test principals and must fail or report a missing prerequisite rather than silently falling back to a fixture.

Acceptance covers PKCE login, refresh, current-user and immutable-org introspection, same-org enrollment under configured policy, unrelated valid-user refusal, guest invitation plus bound approval code, expired/replayed code refusal, membership removal, agent unpublication and user deactivation/token revocation as supported by the deployed contract. Distinguish hub membership revocation from issuer account/token revocation. Record when each denial becomes observable; do not invent an instantaneous revocation guarantee beyond the existing authorization/cache policy. Restore only explicitly owned test resources, never unrelated memberships or users. Account MFA remains personal and separate from project approval codes.

## 2. Browser collaboration

Add a Collaboration view scoped to the selected Location and bound project. Use the local owner API and its account connector rather than exposing the hub backend credential to JavaScript. Show setup/disabled/unauthorized states with actionable diagnostics. Keep feature enablement global and explicit; opening a checkout must not opt it into collaboration.

People identifies human members, published agents, verified owners, roles and observed presence. Conversation offers project and direct messages with authoritative attribution, threads and Git references. Tasks supports create, inspect, claim, update and report using current revisions and claim generations. Existing CLI-only administrative functions can remain linked to their commands unless required for these flows; do not imply the view performs provisioning or account MFA.

Held messages are an explicit receiver inbox, distinct from local tool permission cards. Inspect sender, project, target Session and exact content before release/refusal. The server reauthorizes every release. Capture an immutable action target and keep one operation key through lost acknowledgements. Queued receipts remain visibly queued until reconciled. Clear stale remote content after authorization failure or Location/project switch. Preserve current direct-message visibility and avoid marking remote task reports as verified solve evidence.

## 3. Budget reservations

Introduce a durable reservation ledger keyed by provider request identity, with applicable scope allocations and states reserved, settled, uncertain and explicitly released. An atomic transaction checks recorded spend plus outstanding allocations against all applicable reserved scopes before dispatch. Include descendant Sessions, workflow runs, Goals, Loops, Teams and daily limits wherever applicable, and auxiliary calls such as title, evaluator, advisor and compaction. A caller cannot bypass reservation by changing purpose or spawning a child.

Obtain conservative token/price bounds from the actual normalized request and adapter limits before network I/O. Refuse unbounded or unpriced USD reservations with the existing `BudgetUnpricedError` contract; token-only budgets must still have bounded token estimates. Reserve each token class consistently with the usage accounting contract. If a provider cannot supply the required bound, fail explicitly rather than weakening enforcement. Existing soft budgets retain their behavior.

Settle actual usage and release unused allocations once in the same durable accounting transition. A confirmed pre-dispatch cancellation releases its allocation; interruption after possible dispatch without usage retains an uncertain charge across restart. Add an owner-visible diagnostic and explicit audited reconciliation/release path. Duplicate callbacks, retries and crashes must not double charge or silently free uncertain allocations. Disclose estimated versus actual amounts and overruns; reservation is a scheduling bound, not a provider billing guarantee.

## 4. Transcript retention

Add `storage.retention.workflow_agent_days`, default 30; zero disables this rule. Extend the existing retention worker rather than adding another background scheduler. Prune eligible transcripts only after the configured age since terminal completion, with server-side checks in the deletion transaction.

Protect active/reopened runs, active descendants, pending inbox/permission state, pinned evidence and any data still needed for deterministic replay. Preserve run summaries/results, usage/descendant charges, host verification records, and candidate branches. Record a tombstone so UI/API/export can say the transcript expired under retention instead of showing a fabricated empty conversation. Review foreign keys and event projections before choosing deletion versus redaction. Re-run cleanup safely after a crash; do not leave a projection claiming removed transcript content is still available.

## 5. Cron and persistent local schedules

Complete the existing `loops-scheduling` contracts rather than creating a separate cloud scheduler. Model tools create bounded Session-scoped cron/one-shot tasks; only owner controls create service-wide schedules. The existing `cyber loops` management tree gains the documented persistent schedule operations and matching typed API/SDK operations. Preserve workspace trust, Location and Mode ceilings.

Pin an IANA timezone at creation (default the host's resolved local timezone), show it in every schedule detail, and accept an explicit `--timezone` override. Use five-field cron with a minimum one-minute resolution. A daylight-saving gap skips that local occurrence; an ambiguous repeated local minute fires once at its first occurrence. A later host timezone change does not reinterpret existing schedules.

Persist schedule revision, next due time, occurrence identity, claim and admitted prompt/run receipt. Atomically claim/admit each occurrence; multiple ticks or restart recovery must not enqueue it twice. This does not claim exactly-once external tool execution. Default missed policy is skip; run-once admits at most one catch-up occurrence. Preserve skip/coalesce overlap policy and a bounded backlog. Cancellation/revision fences prevent an old tick from dispatching after pause/remove/edit. Reconcile ambiguous native execution as unknown instead of redispatching automatically.

Persistent schedules default to dont-ask, refuse bypass unless allowed by user/managed policy, and do not create interactive presence. They run only while the local server/service is running. Report last/next occurrence, missed/overlap outcomes, budget stops and permission denials. Cover loops' outstanding hooks, completion notifications, until-goal and exec slash invocation as part of completing the scheduling surface; document platform-specific notification support precisely.

## 6. Browser settings, media and terminal

**Settings:** use config schema, resolved values and source attribution with explicit project/global scope. Offer schema-aware controls and a JSON editor for supported object settings, validation diagnostics and a change preview before PATCH. Managed keys are read-only; global-only transport, identity and feature settings cannot be written to a project file. Never round-trip redaction placeholders into credentials. Secret fields are write-only replacements with explicit clear actions. Never persist drafts containing credentials in browser storage. Show restart-required effects and reconnect behavior before transport changes.

**Attachments:** support only the image/PDF types and limits already admitted by the server/model. Add browser upload and transcript previews using authenticated bounded reads of stored, Session-referenced blobs; never fetch arbitrary transcript URLs or expose arbitrary local paths. Reuse server content-addressed storage, sniffing and modality checks. If a new read route is needed, bind it to Session/blob references with owner authentication, no-store and safe Content-Type handling. Use inert image rendering and an isolated PDF preview or explicit download; reject active HTML/SVG rather than treating it as a trusted document. Revoke object URLs on logout or Session switch and bound decoded/rendered content. No third-party document viewer.

**Terminal:** attach only to existing running PTY Jobs through the existing owner-authenticated ticket and WebSocket API, with a pinned bundled terminal emulator, input, resize and bounded replay. Keep owner credentials out of WebSocket URLs; only the server's short-lived single-use ticket is permitted by the existing contract. Scrub tickets from logs/errors, validate Origin/Host and Location/job authority, and preserve replay cursors without resending input after uncertainty. Disable terminal-originated clipboard writes, remote resource fetching and executable hyperlinks. Distinguish browser disconnect from Job stop; require deliberate stop. Hide unsupported PTY controls on platforms without the capability.

## Delivery and verification

Deliver in dependency order: reviewed contracts and baseline checks; live acceptance harness/provisioning; browser collaboration; reservation ledger; retention; scheduling; browser settings/media/terminal; integrated acceptance and spec archival after merge. Increments may be separate commits/PRs if that keeps review and CI practical. Do not push new implementation directly to main under the earlier documentation-only authorization.

Run focused Rust production-path tests, browser model/transport/render tests, real-server Playwright tests, generated SDK/bundle checks, strict OpenSpec validation and relevant platform CI. Use a scripted local provider for deterministic cost, cancellation, crash and consent cases, and separately label paid/live Auth evidence. Physical-device acceptance must record actual device/browser, trusted certificate hostname, HTTPS connection and a completed request visible from a second client. Browser emulation is not physical-device evidence.
