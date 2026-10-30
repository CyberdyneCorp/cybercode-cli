# Native host integration and Linux support decision

Status: broker direction approved by the user on 2026-10-30. This is the
implementation handoff after the #76 foundations milestone, not production
activation or completed native support acceptance.
The full plugins/hooks, daily-coding and binary-editor scope remains unchanged.

## Approved implementation direction

Continue the descriptor-pinned Linux broker toward production integration rather
than making Landlock ABI 9 the only supported Linux mechanism. Require actual
kernel/LSM capability checks, including exact-thread pidfds and descriptor
duplication; report actionable startup refusal when a required operation is
unavailable. Never substitute a process-leader descriptor table for a requesting
thread, retry without confinement, or report refusal as supported execution.

`PIDFD_THREAD` was introduced in Linux 6.9, according to the
[pidfd_open manual](https://man7.org/linux/man-pages/man2/pidfd_open.2.html).
This is a requirement of this broker strategy, not a claim that every kernel
with that version supports its complete configuration. Backports, LSM policy,
namespace availability and syscall restrictions require feature checks and
positive native startup evidence. The current hosted results cover kernel
7.0.0-1017-azure on x64 and arm64; they do not establish a minimum-kernel matrix.

The alternative is ABI-9 pathname UNIX mediation, documented by the
[kernel Landlock API](https://docs.kernel.org/userspace-api/landlock.html).
Current retained hosted fixtures report ABI 8 and explicitly refuse that
candidate. Selecting ABI 9 as the sole production path now would leave Linux
positive acceptance unproved. It remains an alternative pending real evidence.

This decision approves the implementation direction and its support dependency;
it does not accept the current experiment's compatibility restrictions. In
particular, denying all IP bind/listen or silently limiting private UNIX IPC to
stream socketpairs is not the final capability contract. Private scratch IPC,
existing proxy access and all approved runtimes need positive coverage before
activation. If safe pathname/datagram/seqpacket mediation cannot be implemented
under the approved boundary, return that concrete result for review instead of
quietly reducing supported behavior.

The governing [design](design.md) requires: "If the prototype cannot enforce the
proposed contract on a required platform/runtime, return the evidence and revised
support decision for review before proceeding". No production native activation
or protocol-feature integration crosses that gate before this decision and the
complete required proof have been reviewed. The user selected "Continue the
broker direction" on 2026-10-30 after reviewing this proposal. That closes the
direction decision; the required implementation and acceptance gates remain.

## Production ownership sequence

1. The host registry owns a nonrenewable generation for the exact plugin digest,
   Location and effective trust/configuration incarnation. Revoke before replacing
   or removing that incarnation; fence stale preparation and callback results.
2. Select a reviewed runtime/helper binding independently of project-controlled
   PATH/configuration. Bind executable bytes, dependencies and all macOS signing
   slices. Preserve executable signatures when copying; a target with the helper's
   signing identity is refused before startup. Validate the actual admitted image,
   not an earlier source path that may have changed.
3. Prepare the approved package in the protected owner namespace through the
   trusted no-descendant copy worker. Inventory private roots and aliases without
   opening private regular files in the owner, preserving SQLite-lock behavior.
   Bind roots and protected ancestor identities and reject unsafe links/replacement.
   Staging, scratch, profiles and runtime resources remain outside untrusted writer
   authority for their whole lifetime, including through another admitted plugin.
4. Compose independent read/write/shell grants and the effective network/secret
   intersection. Bootstrap reads do not grant Location/home access. Scratch access
   does not expose another host's scratch, private owner state or proxy authority.
5. Create the narrowed proxy and immutable policy resources. Perform the final
   generation/provenance check and synchronous actual spawn under the commit gate.
   A returned future or copied path cannot serve as that commit.
6. Retain the admitted package, runtime/helper, policy, scratch and proxy in the
   started-host owner until native tree settlement and proxy disposal acknowledge.
   Revoke network authority promptly on cancellation. Unknown wait state retains
   resources and quarantines restart; a fresh generation cannot reuse uncertainty
   as evidence of successful cleanup. Never signal an uncertain cached PID.

The library foundations in #76 implement pieces of this sequence. The production
App's legacy launch path does not implement this sequence today.

## Linux mediation obligations

- Keep private user/mount/PID/network/IPC namespaces, protected mount ancestry,
  dropped capabilities, descriptor sanitation and inherited filesystem policy.
  A private network namespace alone cannot hide pathname UNIX endpoints exposed
  through readable mounts, as the retained host-socket counterexample demonstrates.
- Use one seccomp notification listener with a separate consumed startup-exec
  budget for the exact trusted guest. Subsequent execution with shell disabled
  stays denied; shell-enabled descendants retain the complete boundary.
- Pin the requesting thread's actual socket and copy bounded address arguments
  before deciding policy. Emulate admitted operations on owned objects instead
  of continuing an inspected mutable socket syscall. This follows the
  [kernel notification guidance](https://docs.kernel.org/userspace-api/seccomp_filter.html).
- Extend IPC compatibility without admitting ambient endpoints. Pathname checks
  must bind the actual endpoint and protected directory provenance, withstand
  concurrent replacement/symlinks and cover destination-bearing send operations.
  A string-prefix or realpath-then-connect check is insufficient. The current
  broker does not yet implement these obligations for general private UNIX IPC.
- Preserve effective proxy/network ceilings. Network-denied hosts cannot reach
  ambient host services; host allowlists cannot be bypassed through direct IP,
  UDP, alternate socket families or an unrelated UNIX endpoint. A proxy endpoint
  grant must bind the exact owned endpoint without exposing neighboring sockets.
- Bound workers, retained descriptors, memory, notifications and cancellation.
  Unknown effects are not replayed. Reaping must keep process identity ownership
  through settlement, including escaped descendants and parent/runtime shutdown.

## Implementation and acceptance tasks

- [x] N1 Review the Linux broker/support direction above; record the decision.
- [ ] N2 Complete required native IPC/network compatibility and adversarial proof,
  including private UNIX behavior and proxy endpoint identity. Retain positive
  runtime controls and denied ambient access, not only refusal cases. The
  [endpoint identity prerequisite](linux-unix-endpoint-identity.md) tests the first
  pathname-object binding step; the [private stream candidate integration](linux-private-stream-integration.md)
  applies that primitive in real connect notifications. Neither completes this task.
- [ ] N3 Implement registry-owned generation admission and immutable resource
  binding with actual production startup, revocation and quarantine tests.
- [ ] N4 Integrate the native launch owner and independently intersect all
  filesystem/shell/network/secret capabilities. Remove the legacy plugin launch
  route when accepted native support replaces it; preserve ordinary tool policy.
- [ ] N5 Verify native, Node, Python, Bun and Deno on supported macOS/Linux x64
  and arm64 configurations. Record exact unsupported Windows results and
  actionable diagnostics; no universal-refusal acceptance.
- [ ] N6 Run real App/server tool-execution acceptance, malicious direct native
  API/constructor/descendant cases, aliases and concurrent replacement, startup
  cancellation, crash/restart, trust loss and resource settlement. Include precise
  missing-enforcement controls and relevant full CI plus code/security review.
- [ ] N7 Reconcile the support matrix, original parent tasks and public docs,
  then continue protocol operations, scoped callbacks and remaining producers.

No implementation or acceptance item is completed by this proposal. Tasks 2.1–2.4
and the remaining full-goal work remain open.
