## 1. Review and baseline

Delivery rule updated by the user on 2026-10-29: normally one open goal PR,
maximum two only for independently reviewable work. #70–#75 were consolidated into
#76 against `main`. On 2026-10-30 the user authorized merging that foundations
milestone after green CI and merge review, then continuing in one follow-up PR.
PR #76 merged at `ec31ff9f` after 70 passing checks and one intentional diagnostic
skip. Its tree matches the tested `e6fd3d2f` tree. Continue on the single native
host admission follow-up; the foundations milestone does not complete task 2.
The full goal scope, order and acceptance gates remain unchanged. See
[goal delivery rules](../../../docs/goal-delivery.md).

- [x] 1.1 Inspect canonical plugin/hook contracts, SDK protocol, host confinement and explicit unsupported paths at `097b6278`.
- [x] 1.2 Draft the proposal, design, supplemental scenarios and ordered implementation tasks.
- [x] 1.3 Review proposal/design/tasks with the user before security-sensitive or capability implementation. Record approved scope and platform/runtime decisions. Approved on 2026-10-27: after PRs #53/#54 merged, the user explicitly requested implementation of both merged plans and then instructed goal resume. Native proof, baseline and acceptance gates remain required.
- [x] 1.4 Inventory actual producers and protocol operations; map existing tests and stale parent tasks. Run relevant unchanged-main baseline checks and record outcomes. [baseline.md](baseline.md) records the initial failed compilation and SDK results, followed by 14 real-process plugin tests passing on unchanged main `2ae7957d` on macOS arm64 and the same 14 passing on the integration revision. This completes the baseline inventory, not native capability enforcement or later acceptance gates.

## 2. Native capability enforcement

The user approved the [Linux broker integration direction](native-host-integration.md)
on 2026-10-30, including capability detection of the exact-thread pidfd dependency
and retaining private IPC/network compatibility as an activation requirement.
Only that direction decision is complete; the parent proof, implementation and
acceptance tasks below remain open.

- [ ] 2.1 Prove a native enforcement mechanism and document runtime bootstrap, read roots, writes without implied broad reads, threads, exec/fork/descendant behavior and OS/runtime support. Present any required design revision before integration. Native CI at `59cae487` passed ten jobs (90 cases) covering Node/Python on macOS/Linux arm64/x86_64 and native Linux executables, including pre-main reads/forks and immutable one-shot exec filtering. Evidence and remaining runtime/platform/integration gates: [native-proof.md](native-proof.md); parent task stays open pending the complete support decisions and acceptance evidence.
- [ ] 2.2 Implement plugin-specific confinement, reviewed runtime resolution and descriptor/environment isolation without weakening ordinary tools or protected roots. The content-bound helper copy foundation is documented in [image-preparation.md](image-preparation.md), with packaged macOS entry progress in [packaged-native-entry.md](packaged-native-entry.md) and Linux entry progress in [packaged-linux-entry.md](packaged-linux-entry.md); [Generation-bound final commit](launch-authority.md) adds synchronous revocation/start ordering, and [host bootstrap environment](bootstrap-environment.md) replaces implicit environment inheritance; [Runtime startup isolation](runtime-startup.md) adds package CWD/config selection and prepared workspace resources; [Private-root inventory](private-root-inventory.md) adds bounded metadata-only alias detection; [Native policy composition](native-policy-composition.md) adds the bounded Rust macOS filesystem/process builder and packaged-helper receipts; [Linux protected mount ancestry](linux-mount-protection.md) records five passing primitive cases per Linux architecture, including actual ancestor-move counterexamples and regular-file masks, before [Rust Linux composition](linux-policy-composition.md), which supplies bounded arguments/profile and actual-helper regressions passing on Linux x64/arm64 at `546641f9`; complete owner admission and production native launch integration remain open.
- [ ] 2.3 Enforce `fs.read`, `fs.write`, `shell`, network and secret intersections; refuse unsupported confinement with actionable plugin diagnostics. [Linux IPC diagnostic](linux-ipc-proof.md) proves fixture-owned host socket reachability through the composed production boundary; the [isolated socket-family candidate](linux-ipc-candidate.md) passes its fixture matrix but restricts bind/listen, while the [path-scoped ABI-9 alternative](linux-unix9-candidate.md) tests preserving private IP/UNIX functionality without production activation or an approved support-floor change. The [descriptor-pinned broker experiment](linux-socket-broker-candidate.md) now attempts private IP bind/listen/connect with bounded workers and a one-use startup-exec decision. Native and runtime results plus the support/design gate remain required. None closes this task.
- [ ] 2.4 Add real-process positive/negative tests for native reads, home/Location/symlink bypass, spawning a shell/interpreter/native child, inherited restrictions, protected state, revocation and unsupported hosts. Obtain native macOS/Linux evidence and explicit Windows results/limits.

Runtime-selection follow-up for task 2.2: the [production PATH guard](runtime-search.md)
now excludes project/package directories, relative entries and symlink targets
back into either tree. It is not fixed reviewed runtime/image admission; the
parent native enforcement and support gates remain open.

The [same-group host child cleanup](host-process-lifetime.md) routes crash and
graceful waits through the existing tree-aware Process owner. Live-child and
missing-fix regressions cover those paths; escaped descendants, startup/cancellation
settlement and complete native resource lifetime remain open.

The [cancellation-safe started-host owner](host-resource-ownership.md) now retains
scratch through asynchronous cleanup and unconfirmed waits while revoking its
proxy. Native containment, staging ancestry, quarantine and generation/retry
fencing still gate complete host lifetime acceptance.

The compatible Linux IPC investigation includes [descriptor-pinning
prerequisites](linux-pidfd-proof.md), including independent thread file tables,
descriptor-number replacement, copied arguments and actual private TCP exchange.
The [broker candidate](linux-socket-broker-candidate.md) has source-bound initial
socket/runtime and real thread-local descriptor receipts on x64 and arm64, with
the precise missing-thread control. Its [cancellation case and control](linux-socket-broker-cancellation.md)
also have source-bound native receipts, as do the [post-inspection mutation case and controls](linux-socket-broker-mutation.md).
Its [escaped-descendant namespace case and control](linux-plugin-descendant-proof.md)
also have source-bound native receipts. Its [trusted-worker capacity/recovery case and control](linux-socket-broker-capacity.md)
passed on x64/arm64 in run 41049265434, with source-bound receipts retained. These
experiments change no production policy or accepted support floor.

The [raw datagram send-result correction](macos-send-result-validation.md) retains
raw syscall results and tests the shared verifier helpers with six deterministic
fault-injection cases and a precise missing-guard control. Both corrected hosted
macOS jobs at `62a3068e` passed those controls and the unchanged real IPC matrix.
The control is verifier evidence, not native confinement acceptance. The historical
unknown race cause and production native admission remain open; this correction
does not close tasks 2.1–2.4.

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

- [ ] 5.1 Implement committed `DirectoryAdded`/`CwdChanged`, complete existing `Setup` init/marker semantics and fill proven isolated-child `WorktreeCreate` coverage gaps without moving its pre-effect veto. Isolated-child creation/recreation admission is implemented with regression coverage in `isolated_subagents.rs`; at source commit `b5ff9553`, all 16 broad CI jobs and both Linux/macOS before/after regression jobs passed ([broad CI](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40572548204), [regressions](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/40572548267)). The parent task remains incomplete. Committed CwdChanged is implemented by PR #57; code revision d9932631 passed all 36 focused worktree tests on Linux/macOS plus broader CI. DirectoryAdded/Setup and combined event acceptance remain open.
- [x] 5.2 Implement bounded `FileChanged` observation/reconciliation for tools, formatters, shell/background and external edits, including overflow and self-write loop prevention. Verify alias-aware Session admission and resource/lifecycle behavior before closing this task. See [file-observation-implementation.md](file-observation-implementation.md) for implementation and verification evidence.
- [ ] 5.3 Test no false post-effect success for refused/failed/unknown transitions, pre-effect WorktreeCreate veto/no Git mutation, stable event identity, correct scope/order, rename/delete, multiple Sessions, shutdown/restart and unknown handler outcomes.

The CwdChanged regression
`cwd_changed_failed_idle_exit_commit_is_silent_and_retry_commits_once` in
`crates/cyber-tools/tests/worktree_setup.rs` exercises a real failed idle-exit
append and HTTP hook delivery. It verifies that the fault was reached, the
Location stays unchanged, and no durable exit or CwdChanged appears. After the
fault is cleared, retry commits one exit with the matching hook identity; another
exit is silent. This is a known before-append failure, not proof of an unknown
commit outcome. All five HTTP CwdChanged cases are enabled on Unix and Windows
with unchanged assertions. The Windows CI profile already selects the complete
`worktree_setup` binary, so these cases must execute there. All five pass locally
on macOS; the [merged PR #88 native Windows job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41221552314/job/124597023928)
also executes all five successfully. Its Linux/macOS checks pass as well. This
accepts those cases, not the full producer matrix; task 5.3 stays open.

Task 5.2 is accepted by merged PR #85 (`5d5826b7`). The
[acceptance audit](file-observation-acceptance.md) maps the complete FileChanged
contract to actual native/runtime results, all three final platform jobs, broad
checks and the exact accepted main composition. Earlier byte-budget, private
retirement and pending-owner regressions are included in this acceptance.

The [DirectoryAdded prerequisite proposal](directory-added-design.md) maps the
unimplemented additional-directory owner operation to permission, sandbox and
Context Source publication at a Safe Boundary. Tasks D1–D6 require review and
implementation; no standalone emitter or successful addition is claimed.

Setup ownership is expanded in [setup-design.md](setup-design.md). Its Store
epoch and exact-directory marker choices are proposed for review; marker/schema
and init behavior are not implemented by this specification. Tasks S1–S6 there
expand task 5.1 without closing it or the other producer obligations.

## 6. Acceptance and delivery

- [ ] 6.1 Update plugin/hook guides, capability support matrix, API/SDK/schema artifacts and original extensibility task evidence without blanket-closing parent obligations.
- [ ] 6.2 Run relevant Rust tests, format/Clippy, plugin/SDK checks, native positive/denial tests, strict OpenSpec/lint and real-server acceptance; record configured skips explicitly.
- [ ] 6.3 Perform substantive code/security review using the code-review skill; resolve findings and obtain green relevant CI in reviewable PRs. Merge only when authorized.
- [ ] 6.4 Record accepted first-workstream evidence, then begin the reviewed Daily coding workstream. Archive only after accepted merge and reconcile overlapping deltas.


## FileChanged acceptance after main integration

PR #85 merged after all 19 current-head checks passed. Final source-bound
Linux/macOS/Windows suites pass both retarget regressions, bounded native and
runtime observation, durable feedback, private-state retirement and joined
observer/pending-owner lifetime checks. The accepted merge tree equals the
reviewed composition with PR #88. See the
[acceptance audit](file-observation-acceptance.md) and its final receipts for the
scope, intentional skips and unverified Windows leaked-process warning. This
closes only task 5.2; tasks 5.1, 5.3 and full workstream acceptance stay open.
