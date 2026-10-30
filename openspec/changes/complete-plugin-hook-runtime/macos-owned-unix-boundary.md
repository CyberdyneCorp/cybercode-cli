# macOS owned UNIX listener and peer identity controls

This extends the approved native proof independently of the pending
[Linux revision](linux-kernel-unix-revision.md). It uses the same experimental
profile as the runtime matrix. It does not activate production hosts or close
full private IPC acceptance.

`macos_policy.mts` contains the profile builder shared by `run.mts` and the new
`macos_private_ipc.mts` driver. Extraction preserves the preceding function body
apart from its export; that equality was checked against the preceding Git
source. The runtime source manifest now includes this module, and the
missing-grant control modifies its temporary copy instead of a duplicate policy.

The driver owns a UNIX listener in a fixture Location. It starts the compiled native guest without changing its image/signature,
through the freshly signed pre-runtime launcher, with
descriptor sanitation and the existing exact-target execution rule. The native
guest has no policy installation step. Each of the eight read/write/shell
combinations runs separately after an unconfined positive control.

The guest must exchange a token and acknowledgement through absolute and
relative scratch sockets and through hard-link and symlink aliases of its own
scratch socket. Both ends of all four exchanges call getpeereid and
LOCAL_PEERPID: eight identity checks must match the guest's PID, UID and GID.
This checks actual native caller identity, separately from byte exchange.

The guest then tries the owned host listener directly and through scratch
hard-link/symlink routes. It reports connection, token-send and acknowledgement
results independently. The owner records the complete fixed token before
acknowledging; target-reported failure alone cannot prove isolation. The
unconfined control must create both aliases and deliver three acknowledged
tokens. Every confined case must report no host connection/send/ack and no
actual owner receipt. A refused foreign hard-link creation is recorded as such;
it is not mislabeled as a successful alias followed by denied connect. Positive
private aliases ensure this does not accept a blanket ban on socket aliases.

The missing-scope control widens only the outbound UNIX path grant from scratch
to `/`. It must retain private exchanges/aliases and identity, actually receive
an acknowledgement from the owned host listener, and fail the named
`macOS UNIX socket boundary escaped scratch` assertion. Profile parse failure,
startup refusal or a broken private exchange cannot satisfy that control.

The owner bounds connection count, token buffering, socket waits, subprocess
output and execution time. Each listener closes after the guest settles, all
accepted sockets are disposed, and the owned temporary root is removed. This
fixed-process diagnostic does not replace the production tree owner or prove
arbitrary escaped-descendant settlement.

## Local result

On macOS arm64, Darwin 25.6.0, all nine native listener cases pass. The control
receives three real host tokens; all eight confined cases receive none, retain
all four private exchanges and pass all eight identity checks. Scratch-created
hard links and symlinks work. Foreign hard-link creation is refused in the
confined cases, and created foreign symlinks cannot connect. The widened-scope
control actually reaches the owned listener and fails its exact assertion.

Native C, Node and system Python also pass all 54 runtime cases through the
fixture-built and packaged launchers after the extraction. The shared-policy
missing-grant control passes its unconfined row and fails the exact
no-capabilities exchange assertion.

Original reports and both logs are retained under
`scripts/probes/plugin-confinement/evidence/macos-owned-unix-local`.
The owned-listener source manifest SHA-256 is
`fbe318d5d33b107ef35c988168203d005b87a11c24805f412a27bb65c5511416`;
the runtime manifest SHA-256 is
`b8db40c4efa39e283514b64648916bfb3657e2233f9c7a3c7194be949fe620c1`.
Both were independently recomputed from the actual source inputs.

The cognitive-complexity skill measured byte-identical `.ts` copies because its
walker omits `.mts`: the shared builder scores 13 and the largest emitted new
driver function scores 3, with no analyzer warnings. Native C and TypeScript
syntax checks pass. Hosted macOS Intel/arm64 execution remains pending for this
extension. The preceding [Intel Python job](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41095335363/job/124199509520) passed its runtime assertions but its
artifact upload failed after five service timeouts; that missing report is not
counted as retained acceptance evidence.

Still required: concurrent alias replacement, datagram/seqpacket, ancillary FD
and credential behavior, proxy/network intersections, immutable owner admission,
full resource settlement and actual App/server execution. Native and parent
task checkboxes remain open.

## Hosted result for bf87b7b9

[Run 41098929223](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41098929223)
passes both native signing jobs on macOS arm64 and Intel, Darwin 24.6.0. Each
architecture has nine owned-listener cases and nine native runtime cases.
Private streams, both private aliases and all eight peer identity checks pass;
only the unconfined control delivers its three host tokens. The missing-scope
and missing-grant controls fail their exact assertions.

Original reports and logs are retained under
`scripts/probes/plugin-confinement/evidence/macos-owned-unix-bf87b7b9/{arm64,x64}`.
Both checkout logs identify merge `de3eabe960ec62c2820eea11c56ff208cd48a9ae`;
its tree equals source head `bf87b7b9ecc4e179902b78147fa9fd737c5bf629`.
Both source manifests above were independently recomputed from that Git tree.
A separate binding receipt records these checks without modifying the original
reports. This is native-only hosted acceptance for the named cases, not the full
five-runtime matrix or production support.

The subsequent [private descriptor extension](macos-private-descriptor-passing.md)
adds real SCM_RIGHTS transfer and a pinned owner file-effect check resilient to
guest pathname replacement. The receipts above do not cover that extension.
