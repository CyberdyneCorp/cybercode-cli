# macOS private datagram delivery proof

This extends the approved macOS IPC proof. Production admission remains gated;
the pending Linux direction review and the full goal scope are unchanged.

The native fixture exercises private pathname UNIX datagrams with connected
`send`, destination-bearing `sendto` and destination-bearing `sendmsg`. Each
operation uses a fresh server/client pair, sends the fixed `fixture-only` token,
checks actual server reception and requires a return acknowledgement. All nine
rows also retain private stream/alias exchanges, eight peer identity checks,
SCM_RIGHTS reception and the pinned original file effect.

A separate trusted native datagram receiver binds an owned host endpoint before
the guest starts. The guest tries six routes: direct and scratch-symlink forms of
connect/send, sendto and sendmsg. Each uses the exact owned scratch client path.
The receiver accepts only the fixed twelve-byte token from that path, records
receipt before acknowledging, and bounds packets, output and execution time.
After the guest settles, the parent requests stop; the receiver drains queued
packets and must exit successfully with an explicit final receipt report.
A failed receiver never supplies a valid zero. Child ownership uses the process
handle and cached settlement promise, including cleanup on assertion failure.

Startup controls require a missing executable and a real failed bind to reject.
The failed-bind control also verifies an existing owned file remains unchanged.
These refusals test the recorder; they do not count as positive IPC acceptance.

The unconfined row must receive and acknowledge all six host attempts. Each of
the eight read/write/shell combinations must retain all three private datagram
exchanges, fail all six host attempts and produce zero actual host receipts.
The `--missing-scope` control widens only the outbound UNIX endpoint scope. It
must reach both owned stream and datagram receivers, deliver six datagrams and
fail `macOS datagram endpoint escaped scratch` after the private, peer and
SCM_RIGHTS checks succeed. Dropped-rights and path-reader counterexamples remain
required and fail their separate named assertions.

## Local evidence

On arm64/Darwin 25.6.0, nine extended native IPC rows pass, as do nine native
fixture runtime rows, nine native packaged-helper runtime rows and nine Node
fixture runtime rows. There are no unsupported rows in these selected matrices.
All three precise counterexamples fail as intended. Source manifests were
independently recomputed against the current files before retaining evidence in
`scripts/probes/plugin-confinement/evidence/macos-private-datagram-local`.

The IPC manifest is
`a82c76656620443cc1739f629eda53958a64551f06f1d9bb064d49455c0248d3`;
the runtime manifest is
`9db4c7e189904e31ca2954e8f82e7324e13ca9f44611a0e76039898e7f3a1b96`.
The IPC manifest includes the native receiver and its typed owner module.
The runtime manifest includes the new header compiled by the native fixture.
Hosted results for this extension are pending. Prior green hosted descriptor
results are documented separately in [descriptor passing](macos-private-descriptor-passing.md).

These fixed pathname exchanges do not prove concurrent endpoint mutation,
seqpacket compatibility, ancillary credentials, every descriptor operation,
proxy/network/secret ceilings, complete descendant resource settlement or actual
App/server integration. Those acceptance requirements remain open.

The subsequent [concurrent alias replacement](macos-datagram-alias-race.md)
extends this fixed-route revision with separately retained race evidence.
