# Runtime support revision for review

Status: the availability-reduction proposal below was not approved; the original
strict scope remains required. New signing-identity evidence supports continued
work toward that scope. Task 2.1 and the native admission/integration gates remain
open; no production launcher or capability semantics change here.

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
re-signed x86_64/arm64e system image. At `74ae15f1`, the added direct/unconfined
control terminated with SIGSEGV and the sandboxed control with SIGKILL, both
without output. Their cause and portable staged-image admission remain unresolved;
local success and Intel success do not satisfy that missing positive control.
The proposed availability table and request for an explicit support decision
remain unchanged pending that evidence and review.

## Signing-identity evidence without reducing availability

At `bcad75b3`, both macOS 15 arm64 and Intel signing-identity candidate jobs
passed all nine native capability rows plus original-system-image exec controls.
The target signature stays intact; the trusted bootstrap alone receives a fresh
identifier. The pre-runtime extension at `574714f8` also passes 36 local rows for
native, Node, standalone Python and framework Python. Its new runtime/platform
CI remains separate from the historical staged-image failure. See the exact-source
reports in the [native proof ledger](native-proof.md#pre-runtime-signing-candidate-framework-admission-and-runtime-coverage).

An independent admission experiment now reproduces pre-existing private-file and
package-write aliases despite the path-only policy. A known-file stat guard and
distinct-inode copies demonstrate targeted refusal and successful clean startup;
complete owner inventory, copying/race bounds and host integration are not yet
proven. See [the admission experiment](../../../scripts/probes/plugin-admission/README.md).
Those requirements cannot be replaced with an exec-policy pass or a universal
launch refusal.

## Historical availability-reduction proposal (not selected)

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

## Retained scope and integration gate

The goal retains strict native filesystem/process capabilities, including arbitrary
macOS native executables with shell=false. No approval to reduce that scope has
been recorded. The historical table is not a production availability contract.

Keep the approved native proof/design gate before protocol integration. Before
claiming production support, establish immutable helper/package ownership, reviewed
runtime resolution, private aliases/protected-root composition, network/secret
intersections and real App/server acceptance. If required support
cannot satisfy the original contract, present that evidence and a concrete revised
support decision for explicit review. Experimental positives or targeted unsafe-image
refusals do not close the goal, the remaining host protocol work, Daily coding or
the binary editor.
