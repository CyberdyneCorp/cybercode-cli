# Bounded pending FileChanged owner replacement

Review reproduced a resource-lifetime defect in the existing observer registry.
A cancelled owner could still be settling. Each remove/readmit cycle then created
another Tokio task awaiting the previous task. The registry held one entry while
its nested chain grew: 256 cycles created 257 replacement tasks in the regression.
The new regression executed against unchanged production code at main
`3ecf0755` and failed with 257 replacement tasks. Only the test was added for
that baseline run.

A pending replacement now retains its owner token until its predecessor settles.
Removing a Session still cancels that Session's callback token immediately and
removes its renewal/scope admission. Readmission joins the single pending owner
with a fresh callback token. Once the predecessor settles, activation clears the
pending flag under the membership lock and cancels an empty owner. Admission
checks liveness under that same lock, so it cannot attach an already-cancelled
child token during activation. Shutdown independently cancels and joins owners,
including their retained predecessors. No uncertain resources are discarded.

The regression uses the real registry refresh/remove paths and a controlled held
predecessor. It requires one replacement across 256 cycles, cancellation of every
removed token, fresh live authority after final readmission and revocation on
joined shutdown. The original test executed and failed at the task-chain assertion
with 257 replacements. The corrected file-observation library suite passes 58
tests with one subprocess helper intentionally ignored by the parent test runner.
All 24 actual HTTP callback lifecycle regressions also pass locally. These
component results do not prove native confinement or current-source platform
acceptance. Scoped `cyber-tools --lib` Clippy with warnings denied and Rust
formatting pass. Strict OpenSpec validates all 114 items; spec lint reports zero
errors and 25 warnings.

The focused workflow removes only the pending-owner cancellation guard as a
counterexample. It must execute the regression and fail its exact task-chain
assertion; compilation failure cannot satisfy the control. The exact control
was executed locally and failed at that assertion. Platform CI and composed-feature
acceptance remain required. Task 5.2 stays open, as do the separate plugin native
admission and protocol gates.

## Composed platform verification at 3f7067dd

The [fresh workflow](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41180219218)
completed successfully on Linux, macOS and Windows after the main integrations.
Its exact source is `3f7067dd058b91c11320c99c556b60451b103098`. This supplies
current composed platform results rather than treating the earlier d689d2f3
jobs as evidence for later main changes. Full task-5.2 requirement-by-requirement
acceptance review remains open; native plugin admission is a separate gate.
