## 1. Review and prerequisites
- [x] 1.1 Inspect the existing P3 requirement and current `cyber pr` implementation.
- [x] 1.2 Draft proposal, design and testable subset requirements.
- [x] 1.3 Review proposal/design/tasks with the user before implementation. Approved in this conversation on 2026-10-26; parallel implementation was also explicitly authorized.
- [x] 1.4 Refresh main/open PRs; resolve dependencies on reserved budgets, durable scheduling and admission without duplicating their implementations. Main 89bd4e49 includes merged PR #49; implementation stacks on the reviewed reserved-budget commits 244b1525/360ee4ab and uses its existing admission APIs. Persistent scheduling is not required for the 60-second polling worker.
- [x] 1.5 Verify official GitHub API contracts and credential scopes; record baseline checks. Official REST checks, commit status, reviews/comments and collaborator contracts are linked in the GitHub adapter; offline baseline checks are recorded in implementation evidence.

## 2. Durable subscription and GitHub ingestion
- [x] 2.1 Add versioned records/events, migration, revision fences, idempotency and ownership policy.
- [x] 2.2 Implement bounded GitHub adapter, permission checks, conditional polling, pagination and backoff.
- [x] 2.3 Implement deduplicated trigger ingestion, head fencing, claims, restart recovery and pause/stop transitions.

## 3. Fix attempts and publishing
- [x] 3.1 Integrate isolated worktrees, queued Session admission, unattended policy and enforceable shared resource caps.
- [x] 3.2 Pin verification commands and protected paths; retain real checks and patch evidence.
- [x] 3.3 Implement host-side non-force publisher, durable intents and uncertain-push reconciliation.
- [x] 3.4 Await exact-commit CI; implement consecutive-failure counting, closure/revocation and idempotent exhaustion notices.

## 4. Controls, documentation and acceptance
- [x] 4.1 Add owner API, generated SDK, CLI and TUI controls with visible unknown/pending outcomes.
- [x] 4.2 Add meaningful race, restart, security, sandbox and real-Git regression coverage from the design.
- [x] 4.3 Update README, CLI/config/deployment guide and roadmap with examples and the cloud/fork limitations.
- [ ] 4.4 Run strict OpenSpec validation, applicable tests, formatting, Clippy and platform CI; review using the code-review skill.
- [x] 4.5 Explicitly retain live GitHub mutation acceptance as pending. Controlled disposable local Git remotes, finite offline inference and native macOS sandbox fixtures were used; no live GitHub/provider/Auth/desktop operation was performed.
- [x] 4.6 Publish implementation PR #51 with accurate evidence, stacked on reserved-budget PR #50.
- [ ] 4.7 Archive/sync specs after an authorized merge.
