# Verify agent contributions together

## Why

The project hub coordinates people, agents and tasks, but teammate reports are attributed self-reports. The existing solve workflow compares alternative patches independently and leaves them unmerged. A team needs to verify complementary changes together against one pinned base before a human decides to publish or merge.

## What Changes

- Add a local, durable integration queue accepting owner-selected Git commits and optionally references to authorized hub tasks/publications.
- Pin the repository, base, ordered contributions, verification commands and budget in an immutable attempt manifest. Prepare a combined branch in an isolated worktree, reporting textual conflicts separately from failed integration checks.
- Run owner-selected checks on the combined tree, retain host-owned evidence, and request a separate read-only review before presenting the result.
- Expose queue submission/run/status/cancel/report and explicit PR publication through the workflow CLI/API/SDK and dashboard. Every publication is a deliberate owner action; merging remains the owner's decision.

## Capabilities

- ADDED `verified-integrations` (P2): manifests, queue/attempt lifecycle, isolated composition, check evidence, review and explicit publication.
- ADDED requirements in `workflows` (existing command/API group): integration queue surfaces.
- Reuse worktrees, sandbox, permissions, budgets and verified-workflow evidence infrastructure. A hub reference is attribution/authorization context, not authority to run another person's tools.

## Scope and delivery

Local owner-selected contributions work without a Cyber Account. Hub-linked contributions require real membership checks and fail closed while Auth is unavailable; local-only operation must not become an authentication bypass for remote content. Deliver domain/storage/checking first, then CLI/API/SDK, browser controls and integrated acceptance.

No automatic merge to main, automatic conflict-resolution acceptance, GitHub permission synchronization, hosted runner fleet or claim that passing declared checks proves correctness. The implementation must verify combinations, not merely concatenate individual reports. The user approved the linked designs and requested implementation on 2026-10-24.
