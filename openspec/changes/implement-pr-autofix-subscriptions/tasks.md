## 1. Review and prerequisites
- [x] 1.1 Inspect the existing P3 requirement and current `cyber pr` implementation.
- [x] 1.2 Draft proposal, design and testable subset requirements.
- [ ] 1.3 Review proposal/design/tasks with the user before implementation.
- [ ] 1.4 Refresh main/open PRs; resolve dependencies on reserved budgets, durable scheduling and admission without duplicating their implementations.
- [ ] 1.5 Verify official GitHub API contracts and credential scopes; record baseline checks.

## 2. Durable subscription and GitHub ingestion
- [ ] 2.1 Add versioned records/events, migration, revision fences, idempotency and ownership policy.
- [ ] 2.2 Implement bounded GitHub adapter, permission checks, conditional polling, pagination and backoff.
- [ ] 2.3 Implement deduplicated trigger ingestion, head fencing, claims, restart recovery and pause/stop transitions.

## 3. Fix attempts and publishing
- [ ] 3.1 Integrate isolated worktrees, queued Session admission, unattended policy and enforceable shared resource caps.
- [ ] 3.2 Pin verification commands and protected paths; retain real checks and patch evidence.
- [ ] 3.3 Implement host-side non-force publisher, durable intents and uncertain-push reconciliation.
- [ ] 3.4 Await exact-commit CI; implement consecutive-failure counting, closure/revocation and idempotent exhaustion notices.

## 4. Controls, documentation and acceptance
- [ ] 4.1 Add owner API, generated SDK, CLI and TUI controls with visible unknown/pending outcomes.
- [ ] 4.2 Add meaningful race, restart, security, sandbox and real-Git regression coverage from the design.
- [ ] 4.3 Update README, CLI/config/deployment guide and roadmap with examples and the cloud/fork limitations.
- [ ] 4.4 Run strict OpenSpec validation, applicable tests, formatting, Clippy and platform CI; review using the code-review skill.
- [ ] 4.5 Perform authorized disposable-PR live acceptance or explicitly retain it as pending.
- [ ] 4.6 Publish the implementation PR with accurate evidence; archive/sync specs after authorized merge.
