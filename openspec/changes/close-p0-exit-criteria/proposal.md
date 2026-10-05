## Why
P0 milestones are implemented, but tool goldens, storage and startup measurements, and the complete build matrix still lack reproducible evidence.

## What Changes
- Add checked-in golden tool outputs and a registry coverage guard.
- Add repeatable FULL-durability storage and real-terminal startup probes.
- Build all six supported platform targets in CI.
- Publish measured evidence and reconcile the roadmap without marking pending local-model or CI evidence complete.
- Include reasoning and successful compaction usage/cost in evaluation reports and budgets, with regression coverage.

## Capabilities
### Modified Capabilities
- `harness-evaluation`: reproducible local-core exit evidence.

## Impact
Tests, measurement scripts, CI, evaluation accounting and documentation. The authorized local-model investigation installs candidates and retains their failed outcomes; P1 implementation remains outside this change.
