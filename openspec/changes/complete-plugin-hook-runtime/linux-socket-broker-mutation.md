# Post-inspection socket/address mutation proof

This extends the isolated [broker candidate](linux-socket-broker-candidate.md).
It is fixture instrumentation, not production activation or a support revision.

The target creates an IP socket, duplicates it to retain an independent reference,
then calls bind from a pthread with a full sockaddr_storage-sized address. An
audit-only helper pauses after the real notification's exact-thread socket pin,
address copy, family validation and notification-identity check. It blocks SIGUSR2
before installing the fixture handler and emitting BPIN, avoiding a lost wakeup.
Only the 128-byte bind fixture takes this hook; ordinary candidate cases do not.

The trusted Rust driver matches BPIN to the target's requesting TID before sending
one release byte. The target then replaces the original FD number with a UNIX
pair and overwrites the original address with AF_UNIX. A memory fence precedes
signalling a pidfd for its parent, resuming the paused broker without signalling
a potentially reused PID. The helper must operate on the original IP object and
owned address. The target requires a successful bind and a newly assigned IP
port through its retained socket, while confirming its original FD now denotes
UNIX and its original memory has changed. No arbitrary endpoint or credential is
contacted; all operations occur within the composed private namespaces.

Three copied-helper controls must fail that specific final assertion after the
mutation handshake:

- Re-resolve the FD after the pause instead of retaining the inspected object.
- Re-read the guest address after the pause instead of retaining the owned copy.
- Perform the original operation, then incorrectly reply with CONTINUE for that
  notification, causing the kernel to re-read the changed FD/address.

The last control must show that the original IP object was bound but the guest's
result failed; the first two must show the original object remained unbound.
All controls must confirm both FD and address replacement. A startup failure or
missing handshake cannot count as a successful missing-safeguard counterexample.
The changed UNIX address retains the 128-byte argument length, which exceeds its
valid pathname structure; the fixture does not create an ambient UNIX listener.

The shared bounded audit driver uses a 30-second owner deadline, cleans up and
waits its private-namespace process before asserting, and drains both readers.
The deliberately paused broker is confined to temporary test builds and relies
on that outer deadline. Its markers are source-bound fixture observations, not
an authenticated plugin protocol. The base candidate and production helpers have
no pause or marker hook.

Local checks cover the Rust diagnostic Clippy, portable C syntax (excluding the
Linux branch), twelve temporary builder transformations with compiler interception,
workflow syntax, formatting/diff and OpenSpec. Native x64/arm64 execution of the
new positive case and all three controls is still required. Existing thread and
cancellation receipts cannot establish this mutation proof. Saturation, other
signal/cancellation races, descendants, admission, network/proxy/secret ceilings
and the full native support/design gate remain open.

## Native receipts

[Run 41011901810](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41011901810)
passed on x64 (job 123919430419) and arm64 (job 123919430221). Retained
[x64](../../../scripts/probes/plugin-native-entry/evidence/linux-socket-broker-mutation-d47cfe27/x64/linux-ipc-broker-mutation.json)
and [arm64](../../../scripts/probes/plugin-native-entry/evidence/linux-socket-broker-mutation-d47cfe27/arm64/linux-ipc-broker-mutation.json)
reports each contain four successful post-inspection mutations across read/write
combinations and 32 ordinary socket cases. Each matches the requesting TID to the
post-pin marker, confirms the original object was bound and both originals were
changed, and observes a successful caller result. All three adjacent controls
fail the precise final assertion: re-resolution/re-read leave the original object
unbound; incorrect CONTINUE binds it but fails the guest result.

Merge checkout `893b883655dd7ede255e992a99f99bd0bf1859d2` binds source SHA-256
`0ea7e634b39ae67a292044c4846d939c5deb8e54847f60e3839deafc996d55d8`,
independently recomputed from the checkout. Recorded instrumented helper hashes
are x64 `90d204e45347be22ecbdd361a6c3984c7a3de9dde2afa94375037ad037fca19b`
and arm64 `0ac7234c48e23229db1e78cc0b5bd1184e5a2daa7768eb50ac39aac9f99f797c`.
These source-bound observations establish this controlled mutation case, not all
possible concurrent changes or production admission. Subsequent shared-driver
changes still need fresh native checks.
