## Why
P0 milestones are implemented, but tool goldens, storage and startup measurements, and the complete build matrix still lack reproducible evidence.

## What Changes
- Add checked-in golden tool outputs and a registry coverage guard.
- Add repeatable FULL-durability storage and real-terminal startup probes.
- Build all six supported platform targets in CI.
- Publish measured evidence and reconcile the roadmap without marking pending local-model or CI evidence complete.

## Capabilities
### Modified Capabilities
- `harness-evaluation`: reproducible local-core exit evidence.

## Impact
Tests, measurement scripts, CI and documentation; no P1 scope or local-model installation.
