# Runtime support revision for review

Status: proposed, awaiting the user's explicit decision. This does not close task
2.1, change the production launcher, or approve weaker capability semantics.

The approved design says: “If the prototype cannot enforce the proposed contract
on a required platform/runtime, return the evidence and revised support decision
for review before proceeding.” This review concerns runtime availability only;
all plugin/hook, Daily coding and binary-editor implementation objectives remain.

## Evidence

[Native proof ledger](native-proof.md) contains reproducible commands and exact
source-bound reports. At `59cae487`, all ten native CI jobs passed. At `3c8efc33`,
Node/Python/Deno passed their macOS/Linux architecture combinations, Bun passed
both macOS matrices, and the shell-enabled native macOS subset passed on both
architectures. Both native macOS exec-gap experiments passed their assertions:

- Denying process exec prevents the arbitrary executable's initial launch.
- Permitting that executable's exact path also permits it to re-exec itself.

These results disprove those two proposed exceptions. They do not prove that
another mechanism is impossible. At that revision, no suitable general native-macOS/no-shell
mechanism had been demonstrated by this work. It cannot inherit broader exec
rights just to start successfully.

Bun/Linux initially aborted during runtime initialization. At `d5bee987`, both
architectures passed complete native initial-exec and TSYNC matrices, with stdio
tool controls and protected proc-alias tests; exact-source reports are in the
proof ledger. Production integration acceptance remains required. This evidence
does not resolve the macOS native/no-shell support decision.

A later local arm64 experiment also tested a dynamic platform-subject predicate
with a canonical literal path. It permits the trusted system sandbox launcher
to start the custom native fixture and denies that fixture's self-exec, but a
platform payload (`/bin/bash`) retains the exception and self-execs successfully.
The predicate alone does not prove the general contract. Target admission and
immutable staging would need independent enforcement/proof before integrating
such a mechanism. See the source-bound report in the proof ledger; Intel/native
CI for this extension is pending. This evidence changes neither availability
nor the pending decision.

## Staged-image candidate (not an availability change)

A newer local arm64 experiment re-signs controlled native copies ad-hoc with a
fresh identifier and verifies every architecture slice. It passes all eight
capability combinations plus control, including constructor-time exec/spawn and
actual platform-status queries. Staging a formerly platform-signed `/bin/bash`
also removes the demonstrated retained exec exception. The launcher now removes
inherited descriptors through exec-time default closure, with a regression for
a descriptor above a lowered soft limit. See [source-bound evidence](native-proof.md#staged-native-macos-candidate-and-descriptor-regression).

This is a candidate for satisfying the original native no-shell requirement. It
does not yet establish immutable package/image admission, alias protection,
entitlement compatibility or deployable packaging. Both macOS architectures
now require the new matrix in CI. At `ba2d597e`, both native nine-row matrices
passed, but the separate staged-system-payload positive control failed on the
arm64 runner. The diagnostic confirms SIGKILL before output on Darwin 24.6.0 for the
re-signed x86_64/arm64e system image; a separate direct/unconfined control is
being added to isolate staging compatibility from policy startup. Its cause
and portable admission remain unresolved; local success
and Intel success do not satisfy that missing positive control.
The proposed availability table and request for an explicit support decision
remain unchanged pending that evidence and review.

## Proposed production availability

| Platform | Runtime | Proposal |
|---|---|---|
| Linux x86_64/arm64 | Node, Python, Bun, Deno, native executable | Admit only after production integration proves the strict native filesystem/exec policy and complete owner boundaries |
| macOS arm64/Intel | Node, Python, Bun, Deno | Native boundary after trusted isolated initialization and before plugin code; admit only after production acceptance |
| macOS arm64/Intel | Native executable with `shell: true` | Pre-exec native filesystem policy; children retain restrictions; admit only after production acceptance |
| macOS arm64/Intel | Native executable with `shell: false` | Explicitly unavailable until a separate safe mechanism is proved; do not permit self-exec or change the manifest |
| Windows | Plugin hosts without proven equivalent confinement | Unavailable, as already specified by the approved design; ordinary Windows CLI features remain in scope |

A denied launch must explain the unsupported platform/runtime/capability
combination, keep the plugin inactive, and suggest using a supported runtime or
waiting for native support. It must not recommend or automatically add `shell`
merely to bypass the refusal, retry in the ordinary unrestricted tool sandbox,
or manufacture approval through a permission callback.

For admitted combinations, `fs.read`, `fs.write`, `shell`, network and secrets
remain independent, intersected with owner/policy ceilings. No project/home
reads, inherited owner descriptors, protected-state aliases or later execution
are permitted outside those limits. Runtime selection, immutable package staging,
protected-root composition, private startup configuration, packaged helper
lifecycle and actual App/server acceptance remain implementation requirements;
the prototype table alone is not production support.

## Decision requested

Approve this explicit availability matrix as the support contract for production
integration, or retain the requirement to find a general no-shell mechanism for
arbitrary macOS native executables before integration. Approval of availability
must not be recorded as completion of enforcement, protocol features or either
later workstream. No source capability promise changes until the implementation,
regressions and public documentation agree with the approved decision.
