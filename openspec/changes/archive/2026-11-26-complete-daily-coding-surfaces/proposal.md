## Why

The runtime already has snapshots, staged revert, Session branches, Context Epochs and structured child results, but daily CLI/TUI surfaces remain incomplete. `cyber sessions rewind` is still reserved, non-git snapshots are unavailable, path-scoped rules/context inspection are incomplete, and root prompts cannot use the structured child mechanism. Exec consumes stdin as one prompt rather than a multi-turn JSONL driver.

## What Changes

- Add CLI/TUI rewind with preview and code/conversation/both targets, `/undo`, `/redo` and branch-preserving semantics.
- Add bounded, private, conflict-aware non-git snapshot storage and recovery/retention integration.
- Complete path-scoped rule loading and `/context` backed by authenticated inspection of the effective Context Epoch and pending sources.
- Add root per-prompt structured output, `--output-schema` and bounded validation correction using existing schema infrastructure.
- Add `--input-format stream-json`, prompt/control/reply envelopes, per-prompt results and clean EOF/interrupt behavior while preserving ordinary exec behavior.

## Capabilities

### New Capabilities

None. This completes existing daily-use requirements.

### Modified Capabilities

- `snapshots-checkpoints`: consistent branch-preserving revert and non-git restore.
- `system-context`: scoped rule ownership and safe context inspection.
- `configuration`: additive instruction limits without changing the instructions array.
- `session-runtime`: root prompt structured-result ownership.
- `exec-mode`: structured final output and persistent bounded JSONL input.
- `tui`: rewind/undo/redo and context inspection surfaces.

## Impact

Primary areas: runtime revert/branches/inbox/structured output, snapshot storage and GC, instruction discovery and Context Epoch assembly, CLI/TUI commands, authenticated API/OpenAPI and SDKs. Branch/session exports and older event replay must remain compatible. Existing child structured-output contracts remain intact.

This is the second workstream of the requested goal. The original prerequisite to complete Plugins and hooks was superseded on 2026-11-03 when the user prioritized #93 after merging #105. Begin this approved workstream while retaining unresolved Plugins/hooks dependencies and acceptance gates. This does not include IDE clients, Relay, persistent schedules, new model providers, general configuration import, native computer ports or a new plan/agent framework.


## Recovery implementation follow-up

The 2026-11-05 user request prioritizes finishing recovery/snapshots and all
remaining daily-coding surfaces. Historical Git exclusions are an independent
implementation increment. The concrete [managed keep/rollback and native ownership
protocol](managed-recovery.md), linked from design.md and task 3.0, is proposed for
review before changing recovery admission or legacy native-fence authority. It
retains the original scope and makes the new acknowledgement/reclamation contract
explicit; publishing this document is not approval or implementation of that protocol.
