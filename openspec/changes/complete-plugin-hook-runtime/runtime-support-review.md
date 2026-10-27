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
another mechanism is impossible. No suitable general native-macOS/no-shell
mechanism has been demonstrated by this work. It cannot inherit broader exec
rights just to start successfully.

Bun/Linux's initial-exec candidate aborted during runtime initialization on both
architectures. A native TSYNC bootstrap candidate and diagnostic tracing are
being tested independently. Bun/Linux must not be admitted without a complete
passing native matrix. This review does not waive that verification requirement.

## Proposed production availability

| Platform | Runtime | Proposal |
|---|---|---|
| Linux x86_64/arm64 | Node, Python, Deno, native executable | Admit only after production integration proves the strict native filesystem/exec policy and complete owner boundaries |
| Linux x86_64/arm64 | Bun | Same contract; admit only after a complete native candidate passes, otherwise report unavailable |
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
