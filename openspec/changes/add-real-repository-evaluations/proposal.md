# Real-repository evaluations and separate solve measurements

## Why

The GPT-6.1 Sol pilot measured small Python/Node/shell fixtures once per task. Its transport follow-up passed two selected tasks but does not establish Rust repository performance, repeatability or the benefit of verified solve. PR #48 is merged as `e295f43d`; its final PR CI (`40264593449`, 16 successful jobs) and main CI (`40269542675`) passed.

The user requested this delivery order on 2026-10-26: close PR #48; benchmark real repositories including Rust with repeated trials and separate solve evaluation; implement reserved budgets; finish persistent schedules and transcript retention.

## What changes

- Add a small, versioned real-repository pilot: four historical tasks from two public repositories, including at least two Rust tasks, with three trials per task.
- Start source selection with `BurntSushi/ripgrep` (Rust) and `pallets/click` (Python). Freeze exact upstream base/fix commits, source trees, licenses, prompts, environment and grader controls before live attempts. Historical public fixes are retrospective evidence, with possible training contamination disclosed.
- Reuse the existing evaluation runner for single-agent trials. Add a separately reported strategy for the bundled verified `solve` workflow, using two independent candidates and an independent judge. Independently grade the selected patch; a workflow success or judge recommendation is not a coding pass.
- Retain all attempts, infrastructure failures, native usage, pricing uncertainty, timings and host verification evidence. Preserve all previous pilot artifacts and scores.

## Scope and review

This is a new medium evaluation capability and requires proposal/design/tasks review before implementation under the user's AGENTS.md. The benchmark does not change provider authentication, implement budget reservations, introduce hosted execution, compare competitors, or close the 20-task-per-adapter stable-release gate.

Reserved budgets, retention and schedules reuse the already reviewed `complete-dashboard-and-automation` design (sections 3–5); they are subsequent implementation increments, not new approvals requested by this proposal. Their original unchecked tasks remain open until implemented and verified.

## Impact

Extend `harness-evaluation`, evaluation CLI/reporting and independent grader controls. Prefer the current Rust runner and existing Python grader contract; no new hosted benchmark service. API/SDK updates are required only if implementation reveals an actual public API change, and must be reviewed in the design before coding that change.
