## Why

M2.2 already plans a bundled library, but its five workflows do not solve one task through competing implementations and compare executable verification evidence. The existing durable runtime, model tiers, isolated worktrees and budgets make that a useful first delivery slice: users should be able to request several approaches and receive a reviewable recommendation without writing orchestration code.

## What Changes

- Add the bundled `solve` workflow: independent candidate implementations, host-run checks of immutable candidate revisions, an independent read-only judge, and an evidence report.
- Accept two to four candidate model refs or tiers, plus the task and explicit verification command argv arrays. The default is three candidates using `fast`, `smart` and `verify`; all may resolve to one model, which is disclosed.
- Use a clean, recorded Git base and an isolated managed worktree for every candidate. Preserve candidate branches and diffs; never merge, publish or modify the owner's checkout automatically.
- Make checks authoritative host operations rather than trusting an agent's statement that tests passed. Bind every result to the candidate source digest, command, exit status and retained log.
- Recommend only a check-passing candidate after a separate judge assesses correctness, risk, maintainability and scope. The judge may reject all candidates. Report failures, incomplete coverage, unknown outcomes, actual usage and soft-budget overshoot honestly.
- Reuse M2.2's library discovery, origin/override rules, args validation, HTTP/CLI/SDK listing and TUI Library entry.

## Scope and Dependencies

This is the first verified library slice under M2.2, not a replacement for `review`, `audit`, `migrate`, `research` or `plan`. Implement the shared library foundation from `implement-m2-2-bundled-workflows` sections 1–4 as needed, then `solve`; shipping all five other modules is not a prerequisite. The two changes must keep their shared "Saved and bundled workflows" delta aligned.

No new top-level CLI command is introduced. The existing `cyber workflows run solve --args <JSON> --wait` surface is used. Dashboard integration is a follow-up consumer of the same report; the local web proposal can initially monitor runs without the library being implemented.

## Capabilities

- ADDED `verified-workflows` (P2): candidate isolation, authoritative checks, eligibility/judging, evidence and durable recovery.
- MODIFIED `workflows` → Saved and bundled workflows: add `solve` while retaining all existing scenarios and library behavior.

## Non-Goals

Automatic merging or PR publication; guarantees that passing tests proves correctness; a hard financial reservation system; remote runners; arbitrary mutating script tools; benchmarking claims against other harnesses; implementing the five other modules in this slice.

## Status

Implementation started on 2026-10-23 after PR #17 merged the reviewed proposal. The shared library shipped in PR #18. The solve implementation now includes admission, pinned isolated candidates, host-owned checks and recovery records, independent judging, server-derived evidence, client views and usage documentation. Validation and acceptance evidence are tracked in tasks.md; live paid-model acceptance has not been run and no model-quality parity is claimed.
