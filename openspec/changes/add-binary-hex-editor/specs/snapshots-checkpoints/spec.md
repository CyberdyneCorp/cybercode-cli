## ADDED Requirements

### Requirement: Binary save and rewind receipts
(P1) Successful binary saves SHALL integrate byte-range summaries and protected exact preimages with snapshot/restore evidence without fabricating a text patch. Git and non-git Locations SHALL preserve reviewed restore availability even when ordinary snapshots exclude a binary. Save idempotency SHALL prevent duplicate file-change observations; failed/unknown saves SHALL NOT fabricate success. Restore SHALL check the current file identity/bytes and preserve unrelated edits and unresolved effects under the same mutation/recovery guarantees as other code restoration.

#### Scenario: Undo save after an unrelated binary edit
- **WHEN** Restore Last Save encounters externally changed bytes that cannot be reconciled with the recorded result
- **THEN** it reports a byte conflict and retains both versions rather than overwriting the unrelated edit
