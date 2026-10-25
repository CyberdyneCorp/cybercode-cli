## 1. Review

- [x] 1.1 Inspect the existing sidebar, `/agents` sources/actions, native Stop integration and screenshot capture pipeline.
- [x] 1.2 Draft proposal, design and testable TUI requirements for named reachable Sessions/agents.
- [x] 1.3 Review proposal/design/tasks with the user and record approval before capability implementation. On 2026-10-25 the user explicitly approved PR #45 and requested implementation in the same PR.

## 2. Shared data and bounded overview

- [x] 2.1 Reuse the directory entry/source model; separate shared cached data from overlay filter/editor interaction where necessary.
- [x] 2.2 Coalesce ten-second refreshes across visible expanded sidebar and open directory, retaining context/source generation fencing and auth-loss clearing.
- [x] 2.3 Add grouped, sanitized names/status/provenance, six-entry bound, truthful counts, overflow and loading/stale/unavailable/empty states; exclude current-Session duplication and human/wildcard pseudo-Sessions.
- [x] 2.4 Add collapsible sidebar rows and stable-identity navigation to existing directory; preserve all explicit mutation/attach rules and current sidebar focus behavior.

## 3. Verification

- [x] 3.1 Test local/team/workflow/peer/hub projection, duplicate names, unsafe labels/IDs, hidden rows and fallback titles.
- [x] 3.2 Test refresh coalescing, visible/hidden/collapsed demand, slow/unavailable sources, hub revocation and stale replies after context changes.
- [x] 3.3 Test keyboard/mouse activation, reordered/removed rows, busy editors, native warning visibility and global Stop priority.
- [ ] 3.4 Add production-renderer dark/light/NO_COLOR and docked/drawer/narrow snapshots; run the affected TUI suite, strict Clippy, OpenSpec and CI.

## 4. Documentation and real captures

- [x] 4.1 Extend the existing isolated screenshot fixtures with named local/remote/team entries using local scripted services only.
- [x] 4.2 Capture and inspect real updated TUI PNGs from the new binary; refresh affected gallery assets/captions and reproducible capture instructions.
- [x] 4.3 Add a concise README sidebar image and describe availability, refresh/overflow and `/agents` navigation without claiming a new transport or live hub acceptance.
- [ ] 4.4 Complete code/security review and final current-main integration checks; update acceptance evidence.
- [ ] 4.5 Archive the change and merge its requirements into living specs after PR acceptance/merge.

## Current status

The user approved the design on 2026-10-25. Implementation passed 553 TUI tests (three configured skips), final focused regressions and strict Clippy; independent code review approved the final fixes. Real dark/drawer/light captures were independently inspected, and the gallery assets are refreshed. PR CI verification is in progress. Archive remains a post-acceptance step.
