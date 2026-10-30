## Context and source baseline

Planning baseline: `097b6278`, after documentation PR #52 merged. `cyber-core/src/plugins/capabilities.rs` parses all capability booleans. `cyber-tools/src/plugin_host/launch.rs::confinement` currently adds Location reads without inspecting `fs_read`; Seatbelt permits file reads, process execution and forks, while bubblewrap exposes the host read-only by default. `plugin_host/mod.rs` reports hooks/providers/catalogTransforms unsupported and refuses `session/prompt`. The kit already implements tool execution, hook handling and provider streaming, but initializes an empty catalog-transform list.

The existing `implement-m1-3-extensibility` change remains the evidence ledger. This change supplies a bounded delivery sequence and additional contracts; it does not declare its broader parent tasks complete.

The follow-up [source/test baseline](baseline.md) records existing producer corrections and actual check outcomes.

## Decisions

### 1. Enforce capabilities in the process boundary

Introduce a plugin-specific confinement description instead of widening the ordinary command sandbox. Separate runtime bootstrap reads (reviewed executable, loader/runtime dependencies and immutable plugin package), private writable scratch, approved Location reads, approved Location writes, network, subprocess execution and secrets. `fs.write` must not silently imply broad `fs.read`: determine the minimum metadata/read access necessary for writes, document it, and test it separately. Neither flag exposes protected owner state, provider credentials, other plugin scratch or private artifacts. Scratch remains readable/writable to its owner.

With `fs.read: false`, plugin code cannot read project content or arbitrary home files through native filesystem calls, aliases, symlinks or inherited descriptors. With `shell: false`, it cannot launch a shell, another executable, or another copy of its interpreter to evade the restriction. A host's initial trusted bootstrap is not permission for later execution. With `shell: true`, children inherit the same filesystem/network restrictions. Permission callbacks cannot widen manifest capabilities. Launch uses a fixed reviewed executable and sanitized environment; project PATH entries cannot supply the runtime.

**Early implementation gate:** prove the supported OS/runtime enforcement mechanism with a small real-process matrix before integrating protocol features. Current Seatbelt/bubblewrap policy is insufficient for the subprocess distinction. Evaluate a native restricted launcher/bootstrap with enforcement carried across the complete process tree; do not rely on Node/Python wrappers or plugin cooperation. Record the exact OS primitive, initial-execution exception, threading behavior and native tests. The mechanism is deliberately not asserted as already proven. If the prototype cannot enforce the proposed contract on a required platform/runtime, return the evidence and revised support decision for review before proceeding; do not substitute weaker semantics or call universal refusal feature completion.

Windows configurations without proven confinement remain explicitly unavailable. macOS/Linux supported combinations require positive startup/tool-execution controls as well as denial tests; platform build success alone is insufficient.

### 2. Own all contributions by an immutable generation

Normalize initialization into a validated contribution registry keyed by Location, plugin identity/digest and host generation. Validate names, schemas, collisions, protocol compatibility and contribution counts before exposing anything. Disable, removal, content change, trust loss and crash revoke the old generation before any later dispatch. Late callbacks/results cannot republish it. In-flight work retains explicit completion/failure/unknown state rather than being retried merely because a host restarted.

Runtime hook contributions enter the existing plugin scope after managed/global/project/local handlers, respecting managed-only policy and existing timeouts/decision validation. Prevent accidental duplicate registration of the same declared contribution. Independently configured static handlers retain their existing ordering and multiplicity.

Providers use the shared adapter/event normalization, model identity, cancellation and usage path; they never receive core provider credentials implicitly. A plugin provider cannot claim an authoritative reserved bound merely by supplying an estimate: reserved admission refuses until a separately trusted bound contract exists. Catalog transforms operate deterministically on a bounded copy, under stable plugin order, preserving policy and configured overrides. Validate the transformed catalog before replacing the live view; a bad transform cannot corrupt the last valid catalog. Complete the kit's transform API and protocol fixtures together.

### 3. Bind callbacks to real invocation authority

`session/prompt` requires the declared capability and a live core-issued invocation context. Bind it to the originating plugin generation, Session and Location, retain an operation identity across lost acknowledgements, and route admission through the normal inbox/permission/Mode/trust/budget/recursion boundaries. It cannot select arbitrary other Sessions, elevate the Mode or manufacture human attachment. Bound recursive hook/prompt cycles. Calls outside an authorized context fail explicitly. Do not hold a hook/model/tool lock while waiting for a callback that needs the same actor; queue a durable admission and report its admission status, then let ordinary dispatch run it. Clarify permitted callback shapes in SDK types before implementation.

Provider and hook cancellation must settle their owned contexts; cancelled or unknown plugin effects are never automatically replayed. Redact plugin logs/errors and retain only bounded evidence, with no secret-bearing callback payload in telemetry.

### 4. Produce hooks from committed lifecycle changes

Inventory every declared P1 event and its existing producer before adding code. Use the existing event envelope and runtime hook executor. `DirectoryAdded` follows an effective user-authorized working-directory addition; `CwdChanged` follows a committed Session Location change; `Setup` already runs observationally after first root-Session creation; complete its explicit init path and reviewed installation/Location marker semantics, including concurrent first creation and crash behavior. `WorktreeCreate` already runs after permission and before Git effects, and can veto creation. Preserve that ordering, audit child/team/Goal/workflow paths and fill only proven gaps; it is not an after-success event. Failed/prepared/unknown transitions do not produce successful post-effect lifecycle observations; the pre-effect creation hook may legitimately precede a failed setup.

`FileChanged` covers successful built-in/formatter mutations, observed shell/background changes and externally observed changes under the Location. Use bounded, cancellable native observation reconciled against recorded file state; exclude private Cyber state and avoid following symlinks out of scope. A filesystem watcher is an observation, not proof of writer identity or permission. Coalesce noisy observations, bound queues, expose overflow/reconciliation, and prevent hook self-write feedback loops. Do not promise exactly-once execution for arbitrary command hooks: stable event identity and durable status distinguish duplicate observation from an unknown side effect after crash. Recovery retains unknown outcomes rather than blindly rerunning mutations.

## Delivery and verification

On 2026-10-30 the user approved continuing the Linux descriptor-pinned broker
direction with runtime capability checks for its exact-thread pidfd dependency.
The [native-host integration handoff](native-host-integration.md) records this
decision, ownership sequence and remaining IPC/network compatibility obligations.
This approves the strategy rather than production activation or reduced private
IPC support. Required complete native proof and production acceptance remain.

Deliver separate reviewable increments: confinement proof and enforcement; generation registry/runtime hooks; providers/transforms; scoped prompt callbacks; missing event producers. Each increment includes regression tests, docs and scoped tests/Clippy. Use real processes and OS denial/positive controls, a real server plus scripted provider, cancellation/restart tests, and SDK protocol tests. Run complete relevant native checks before acceptance. A code/security review must examine process escape paths, callback deadlocks/recursion, secret handling, late generations and unknown effects. Do not archive the broader extensibility change merely because this slice is complete.

### File observation Session scopes

The concrete provenance and native-lease integration is recorded in [file-observation-scope-design.md](file-observation-scope-design.md). Scope records remain metadata-only; alias acceptance requires live pinned native proof outside deterministic projectors. This implements the approved FileChanged producer scope and does not revise the separate plugin capability-support decision gate.
