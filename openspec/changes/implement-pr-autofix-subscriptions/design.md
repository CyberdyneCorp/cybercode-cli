# Design

## Execution and ownership

A subscription belongs to an authenticated local owner, Location, canonical GitHub repository ID, PR number and linked Session. Resolve repository identity through the configured GitHub connection; do not execute arbitrary PR URLs. Validate current repository write permission, same-repository head, open PR state and workspace trust at enable time and again before execution and push. Never store credentials in subscription rows or logs. Revoked authorization pauses the subscription.

Use a narrow GitHub port with bounded authenticated HTTP requests to the configured GitHub host. Implement production transport and a deterministic fake. Before coding transport, verify current official GitHub API contracts for check runs, commit statuses, review comments, pagination and conditional requests. Credentials stay in the host-side broker; model and verification sandboxes receive none.

## Polling and durable admission

Poll every 60 seconds with conditional requests, bounded pagination and rate-limit/backoff handling. Persist cursors only in the same transaction as corresponding trigger records. Initial enable establishes a review cursor and queues currently completed failing checks once. Read all pages before advancing a watermark; inaccessible or truncated results are surfaced as incomplete, never silently skipped.

Deduplicate CI by repository, PR, head SHA, check/status identity and completion revision; reviews by immutable comment ID plus content revision. New reviews trigger only when their author currently has repository write permission; bot/self feedback never triggers. Review edits retain immutable snapshots. Aggregate one bounded batch per head and serialize attempts per subscription. Revalidate the current head before admitting an event as queued Session input. Stale CI events are superseded; current review feedback can be rebased explicitly onto the new head. Failed checks on a previously unseen human commit can create a new batch; previously seen evidence never starts work again.

Persist subscription revisions, trigger identities, attempt IDs, head SHAs, worker leases, Session admission IDs and mutation receipts. Claims are transactional. Recovery reuses the same admission identity; an uncertain model/tool outcome is held for owner inspection rather than blindly repeated. A lease expiry alone cannot authorize repeating an external action.

## Attempt and push protocol

Each attempt starts in a dedicated worktree pinned to the exact observed head. The worker admits bounded untrusted feedback with `delivery: queue` through the existing admission mechanism; it does not fabricate interactive presence or grant permissions. Fix attempts cannot access GitHub write credentials, other worktrees or subscription controls. Unattended permission requests pause with an actionable status.

Verification commands are owner-configured argv arrays pinned with the subscription revision, run without credentials under the existing sandbox and network policy. Refuse changes to workflows, verification commands and protected policy files in this first release. Require a nonempty patch, passing configured checks and a retained evidence record before a push. Model-generated claims of success are insufficient.

A host-side publisher validates the permitted file set and commit ancestry, rereads PR state/authorization and current remote head, and performs an ordinary non-force fast-forward push. Never use force-with-lease, rebase another actor's commits, or merge the PR. Persist push intent before dispatch. On timeout/restart query the remote branch and exact intended commit: matching history confirms the receipt; a conflicting head pauses; unresolved state remains unknown without another fix or push. Audit the actor, trigger, parent head, resulting commit, checks, costs/usage and uncertainty without tokens or raw logs.

## Limits and outcome semantics

Default maximum is three consecutive failed attempts. A push is not success: await required CI on that exact commit. Pending checks consume no additional attempts. Failed verification, unusable patches and confirmed failed CI count once; transport uncertainty pauses and does not reset the count. Reset only after verified successful CI; lack of check information remains awaiting evidence. Pin a maximum elapsed time and token cap before enable; require at least one enforceable non-dollar cap even for unpriced subscription models. Optional dollar caps require reserved priced request bounds; never describe soft limits as hard caps. Descendants and auxiliary calls share the attempt envelope.

Pause on exhausted limits, unknown side effects, branch movement, permission loss, unsupported bounds or policy refusal. Stop on merge/close or explicit disable. Closing a terminal leaves the durable subscription intact while the server remains available. Disabling fences queued work and cancels active work where possible; an already dispatched external action can still complete and must be reconciled visibly. Post one idempotent exhaustion notice to the PR; uncertain notice delivery is reconciled, not blindly duplicated.

## Surfaces and storage

Extend `cyber pr <n> --autofix` with explicit model, verification and limit options for first enable; add `cyber pr <n> --autofix-status`, `--autofix-pause`, `--autofix-resume` and `--autofix-off`. `/autofix on|off|pause|resume|status` uses the linked Session and the same server contract. Exact route schemas and migration numbers are selected after inspecting concurrent budget/schedule branches to avoid collisions.

Owner-authenticated Location-scoped operations use revision fences and idempotency keys, existing Origin checks and structured errors. Status includes server availability, next poll, trigger revision, attempt stage, check evidence, remaining limits and unknown outcomes. SDK is generated from OpenAPI. README and the CLI guide distinguish self-hosted availability from planned cloud failover and show a complete bounded example. Do not claim the full P3 target is complete.

## Verification

Use fake GitHub, fake clocks and scripted model/tool adapters for duplicate/out-of-order events, pagination, rate limiting, restart at every dispatch boundary, concurrent workers, PR closure, revocation, malicious logs/comments, stale heads, forged check success, failed verification, token/time limits, unknown usage and lost push acknowledgements. Real temporary Git remotes prove non-force push fencing and reconciliation. Test public API/CLI/SDK/TUI controls and retention of accounting evidence. Run relevant platform CI. Live GitHub acceptance uses a disposable explicitly authorized PR after local checks; record acceptance as pending until performed.
