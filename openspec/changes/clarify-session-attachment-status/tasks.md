## 1. Review

- [x] 1.1 Inspect execution-state listing, current sidebar projection, exact-stream presence and existing interactive attachment leases.
- [x] 1.2 Record the user-approved intent and prepare proposal/design/requirements/tasks.
- [ ] 1.3 Review these artifacts with the user before implementing the new attachment field/lifecycle.

## 2. Implementation after review

- [ ] 2.1 Add a read-only exact-scope attachment observation and additive optional reachable-listing field; retain unknown for unsupported/unmanaged data.
- [ ] 2.2 Add generic TUI lease create/renew/release with generation fences, bounded teardown and support independent of computer use.
- [ ] 2.3 Share execution/attachment formatting between directory and sidebar, preserving provenance, source freshness, fair limits and native Stop.
- [ ] 2.4 Synchronize OpenAPI and SDK types and update user-facing lifecycle documentation.

## 3. Verification

- [ ] 3.1 Test live/released/expired/multiple leases, restart and same-Session/different-Location isolation; prevent observer streams from becoming attachment proof.
- [ ] 3.2 Test normal exit, crash expiry, switch/late callbacks, older routes/fields and unavailable sources without false offline/attached claims.
- [ ] 3.3 Test attached-idle, background-running, saved-inactive, waiting and unknown rendering/navigation; retain native/approval fencing and continuing background work/history.
- [ ] 3.4 Run affected Rust/API/SDK checks, strict Clippy, OpenSpec and PR CI; review code and inspect refreshed real screenshots.
- [ ] 3.5 Record acceptance evidence; archive after acceptance/merge.
