Status: awaiting owner review. No task starts before approval.

## 1. Review

- [ ] 1.1 Owner reviews proposal, design and the `snapshots-checkpoints` delta and answers the open questions in design.md.

## 2. Implementation (after approval)

- [ ] 2.1 Read-only stage review (identity, digest, fences, Location state) and fingerprint, for claim, acknowledgement and cancellation-record stages.
- [ ] 2.2 Confirm with the fencing order in design.md, durable intent, no-replace quarantine rename, unlink and completed receipt; resume and lookup by `request_id`.
- [ ] 2.3 HTTP routes, OpenAPI, SDK, `cyber sessions recovery-discard`, TUI entry, `docs/rewind.md`.
- [ ] 2.4 Tests: refusal for every case in "What still refuses"; real process death between intent, rename and unlink; stale fingerprint; removed Location; worktree removal admitted only after discard.
