## 1. Review and baseline

- [x] 1.1 Inspect canonical plugin/hook contracts, SDK protocol, host confinement and explicit unsupported paths at `097b6278`.
- [x] 1.2 Draft the proposal, design, supplemental scenarios and ordered implementation tasks.
- [x] 1.3 Review proposal/design/tasks with the user before security-sensitive or capability implementation. Record approved scope and platform/runtime decisions. Approved on 2026-10-27: after PRs #53/#54 merged, the user explicitly requested implementation of both merged plans and then instructed goal resume. Native proof, baseline and acceptance gates remain required.
- [ ] 1.4 Inventory actual producers and protocol operations; map existing tests and stale parent tasks. Run relevant unchanged-main baseline checks and record outcomes. Source findings and the failed Rust compile/SDK results are recorded in [baseline.md](baseline.md); this task remains open for a passing native baseline.

## 2. Native capability enforcement

- [ ] 2.1 Prove a native enforcement mechanism and document runtime bootstrap, read roots, writes without implied broad reads, threads, exec/fork/descendant behavior and OS/runtime support. Present any required design revision before integration.
- [ ] 2.2 Implement plugin-specific confinement, reviewed runtime resolution and descriptor/environment isolation without weakening ordinary tools or protected roots.
- [ ] 2.3 Enforce `fs.read`, `fs.write`, `shell`, network and secret intersections; refuse unsupported confinement with actionable plugin diagnostics.
- [ ] 2.4 Add real-process positive/negative tests for native reads, home/Location/symlink bypass, spawning a shell/interpreter/native child, inherited restrictions, protected state, revocation and unsupported hosts. Obtain native macOS/Linux evidence and explicit Windows results/limits.

## 3. Host contribution lifecycle

- [ ] 3.1 Validate initialize contributions and add plugin/digest/Location/generation ownership, collision handling, atomic replacement and late-result fences.
- [ ] 3.2 Dispatch runtime hooks through the existing scoped hook engine with decision validation, managed-only behavior, cancellation and bounded recursion.
- [ ] 3.3 Integrate plugin provider streams with adapter normalization, cancellation, usage/accounting, price uncertainty and refusal of untrusted reserved bounds.
- [ ] 3.4 Implement bounded deterministic catalog transforms and the missing TypeScript kit API; preserve configured/policy ceilings and last-valid catalog state.
- [ ] 3.5 Test host crash/restart, disable/update/trust loss, stale callbacks, malformed streams/transforms, duplicate contributions, cancellation and accounting through production paths.

## 4. Session prompt callbacks

- [ ] 4.1 Specify SDK request/result envelopes and live invocation ownership; implement generation/Session/Location-bound durable admission, retry identity and explicit refusals outside context.
- [ ] 4.2 Apply inbox, permissions, Mode, workspace trust, budget, recursion and non-interactive rules without manufacturing presence or blocking the actor needed to admit the prompt.
- [ ] 4.3 Test successful owned callback, duplicate delivery, cross-Session/stale-generation refusal, hook cycles, unattended approval, cancellation, deadlock prevention and crash recovery.

## 5. Missing event producers

- [ ] 5.1 Implement committed `DirectoryAdded`/`CwdChanged`, complete existing `Setup` init/marker semantics and fill proven isolated-child `WorktreeCreate` coverage gaps without moving its pre-effect veto. Isolated-child creation/recreation admission is implemented with regression coverage in `isolated_subagents.rs`; at source commit `b5ff9553`, all 16 broad CI jobs and both Linux/macOS before/after regression jobs passed ([broad CI](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40572548204), [regressions](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40572548267)). The parent task remains incomplete. DirectoryAdded/CwdChanged/Setup and broader event acceptance remain open.
- [ ] 5.2 Implement bounded `FileChanged` observation/reconciliation for tools, formatters, shell/background and external edits, including overflow and self-write loop prevention.
- [ ] 5.3 Test no false post-effect success for refused/failed/unknown transitions, pre-effect WorktreeCreate veto/no Git mutation, stable event identity, correct scope/order, rename/delete, multiple Sessions, shutdown/restart and unknown handler outcomes.

## 6. Acceptance and delivery

- [ ] 6.1 Update plugin/hook guides, capability support matrix, API/SDK/schema artifacts and original extensibility task evidence without blanket-closing parent obligations.
- [ ] 6.2 Run relevant Rust tests, format/Clippy, plugin/SDK checks, native positive/denial tests, strict OpenSpec/lint and real-server acceptance; record configured skips explicitly.
- [ ] 6.3 Perform substantive code/security review using the code-review skill; resolve findings and obtain green relevant CI in reviewable PRs. Merge only when authorized.
- [ ] 6.4 Record accepted first-workstream evidence, then begin the reviewed Daily coding workstream. Archive only after accepted merge and reconcile overlapping deltas.
