## 1. Review

- [x] 1.1 Inspect the existing sidebar, `/agents` sources/actions, native Stop integration and screenshot capture pipeline.
- [x] 1.2 Draft proposal, design and testable TUI requirements for named reachable Sessions/agents.
- [ ] 1.3 Review proposal/design/tasks with the user and record approval before capability implementation.

## 2. Shared data and bounded overview

- [ ] 2.1 Reuse the directory entry/source model; separate shared cached data from overlay filter/editor interaction where necessary.
- [ ] 2.2 Coalesce ten-second refreshes across visible expanded sidebar and open directory, retaining context/source generation fencing and auth-loss clearing.
- [ ] 2.3 Add grouped, sanitized names/status/provenance, six-entry bound, truthful counts, overflow and loading/stale/unavailable/empty states; exclude current-Session duplication and human/wildcard pseudo-Sessions.
- [ ] 2.4 Add collapsible sidebar rows and stable-identity navigation to existing directory; preserve all explicit mutation/attach rules and current sidebar focus behavior.

## 3. Verification

- [ ] 3.1 Test local/team/workflow/peer/hub projection, duplicate names, unsafe labels/IDs, hidden rows and fallback titles.
- [ ] 3.2 Test refresh coalescing, visible/hidden/collapsed demand, slow/unavailable sources, hub revocation and stale replies after context changes.
- [ ] 3.3 Test keyboard/mouse activation, reordered/removed rows, busy editors, native warning visibility and global Stop priority.
- [ ] 3.4 Add production-renderer dark/light/NO_COLOR and docked/drawer/narrow snapshots; run the affected TUI suite, strict Clippy, OpenSpec and CI.

## 4. Documentation and real captures

- [ ] 4.1 Extend the existing isolated screenshot fixtures with named local/remote/team entries using local scripted services only.
- [ ] 4.2 Capture and inspect real updated TUI PNGs from the new binary; refresh affected gallery assets/captions and reproducible capture instructions.
- [ ] 4.3 Add a concise README sidebar image and describe availability, refresh/overflow and `/agents` navigation without claiming a new transport or live hub acceptance.
- [ ] 4.4 Complete code/security review and final current-main integration checks; update evidence and archive only after implementation/acceptance is complete.

## Current status

Specification only. No sidebar capability code or refreshed screenshots have been implemented in this change. User design review (1.3) is pending.
