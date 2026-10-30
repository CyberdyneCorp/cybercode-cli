# Linux UNIX mediation revision for review

Status: proposal for review, not an approved support change or production
activation. The original plugins/hooks, daily-coding and binary-editor scope
remains required. The previously approved broker direction remains the recorded
decision until the user reviews this revision.

## Problem and evidence gate

The [peer-identity diagnostic](linux-private-peer-identity.md) confirms a caller
PID mismatch on both Linux x64 and arm64 in
[run 41092065028](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41092065028).
All four read/write combinations exchange real data, but the listener sees PID 4
instead of caller PID 3. Positive unconfined controls report the actual caller
at both ends. Source-bound reports explicitly mark compatibility unaccepted;
their source manifest and tested-merge/tree equality have been verified.
Successful exchange alone is insufficient.

The current hosted Linux kernels expose Landlock ABI 8. The existing ABI-9
alternative explicitly refuses there; it has no positive native acceptance on
those hosts. This proposal does not turn that refusal into support evidence.

## Recommended direction

Evaluate kernel-enforced pathname UNIX policy as the Linux path to full private
IPC compatibility. Require actual support for Landlock's RESOLVE_UNIX right
(ABI 9 or later), socket creation restrictions, private namespaces and the other
existing filesystem/initial-exec boundaries. Admit only after feature checks and
positive execution establish the complete contract. This changes the proposed
Linux capability dependency and therefore requires review; it does not weaken
IPC behavior or authorize production activation.

UNIX connect and destination-bearing sends execute in the guest under kernel
endpoint enforcement, preserving native caller context. Retain private network
and IPC namespaces, dropped capabilities, descriptor sanitation and immutable
inherited execution restrictions. Preserve real private IP bind/listen and
stream/datagram/seqpacket behavior. Host-facing traffic continues only through
the exact owned proxy and its effective allowlist/secret intersection.

Do not inspect a descriptor in a notification and then CONTINUE merely because
it was UNIX at inspection time. A concurrent replacement could change the object
before the real syscall. Any direct syscall path must have its complete boundary
enforced at actual execution, including the effective network ceiling.

The existing ABI-9 experiment is a starting point, not a ready implementation.
Its profile validation, socket provenance, owner lifetime, complete endpoint
policy and proxy composition still need implementation and adversarial review.
Keep production App activation and protocol-feature integration behind the
existing native proof/design gate.

## Implementation and acceptance sequence

- [ ] R1 Review this changed Linux dependency and direction with the user.
- [ ] R2 Provision isolated Linux x64 and arm64 test environments with actual
  required kernel/LSM features. Pin and record kernel/image inputs and verify
  their provenance. Record ABI and enabled features at execution; do not infer
  support from a version string. Do not replace the user's host kernel.
- [ ] R3 Require direct native positive exchanges for pathname and abstract
  stream/datagram/seqpacket sockets, pairs, named sendto/sendmsg, reconnect,
  relative and symlink forms, ancillary FD/credential passing and peer identity.
  Pair positive unconfined controls with denied actual host-listener delivery.
- [ ] R4 Add concurrent pathname/FD/address replacement and inherited descendant
  cases. The kernel must authorize the endpoint used by the actual operation.
  Omitting the exact UNIX policy must reach an owned host endpoint and fail its
  named isolation assertion; generic launch failure cannot satisfy the control.
- [ ] R5 Run Node, Python, Bun, Deno and native runtime capability matrices on
  both architectures, including real tool execution and private IPC. Verify
  independent read/write/shell grants, effective proxy/network/secret ceilings
  and resource settlement. Retain source-bound reports and exact controls.
- [ ] R6 Return complete evidence for support/activation review, then finish the
  existing N3–N7 production ownership, integration and acceptance tasks. Update
  README/support documentation in the same production behavior change.

Older kernels without proven equivalent enforcement remain explicitly unsupported
for this proposed path. Startup refusal must explain the missing feature and
cannot retry in an ordinary tool sandbox or count as positive acceptance.
No required runtime, private IPC behavior or other goal feature is removed.

## Alternative if older-kernel support remains required

Investigate a protected filesystem endpoint view that excludes foreign sockets
while allowing the guest's own native operations. This must preserve required
file visibility/write semantics and live changes, exclude aliases and concurrent
foreign endpoint insertion, retain private process/network isolation and prove
the same IPC/runtime contract. The existing direct bind-mounted Location is not
such a view. This is a separate, more extensive design/proof effort, not a reason
to accept worker identity or limit IPC to the current stream candidate.

Neither route is complete. The original parent task checkboxes remain open.
