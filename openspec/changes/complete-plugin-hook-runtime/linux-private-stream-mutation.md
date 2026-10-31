# Real broker private-endpoint replacement case

This extends the approved broker work and the
[private stream integration](linux-private-stream-integration.md). The standalone
endpoint prerequisite cannot prove that the real notification path retains its
endpoint handle. This case exercises that path with an actual blocked caller.
Production activation and the complete native/IPC requirements remain open.

The target binds a private scratch stream server and retains an independent
O_PATH reference to its inode. A pthread reports its actual TID, then connects
using a full sockaddr_un and a client descriptor duplicated at a high slot.
The temporary helper's audit hook pauses after copying arguments, pinning the
exact thread's socket and pathname endpoint, and validating the notification.
The fixed-fixture driver releases mutation only after the ready TID matches the
real helper BPIN event. SIGUSR2 is masked before publishing that event.

The target replaces the descriptor number with an IP socket, changes the address
buffer to the fixture-owned host endpoint, unlinks the original private pathname
and replaces it with a symlink to that host endpoint. It verifies a different
pathname inode and that the retained original inode is now unlinked. It signals
the paused helper through a pinned parent pidfd, then joins the caller thread.

The positive case must complete through the original client object and original
private server, exchanging a token and acknowledgement through real sockets.
The unchanged host listener must receive no connection/token. Descriptor,
address and pathname changes, original-inode retention and successful syscall
completion must all be observed; refusal is not a passing result.

The precise copied-helper control closes only the endpoint reference after the
pause. Its original socket and copied address remain pinned/owned. Re-resolving
that pathname must actually deliver the fixture token to the host listener,
with no private-server roundtrip. The driver records that real host receipt
after all fixture processes and pipes settle, then fails the named endpoint
assertion. An unrelated error, missing mutation or absence of actual host
delivery cannot satisfy the control.

All endpoints are fixture-owned, and only the fixed fixture emits the audit
markers; this is not an authenticated production protocol. The shared owner keeps
its existing bounded readers, deadline and cleanup-before-assertion path. The
matrix adds four real mutation cases across read/write combinations, plus the
32 ordinary private-stream cases. Existing cancellation, IP mutation, escaped
descendant and saturation cases still exercise the changed shared driver.

Local Rust example Clippy, portable C fixture syntax, Python/workflow syntax,
18 compiler-intercepted temporary source transformations, formatting/diff and
strict OpenSpec passed before push. These local checks do not compile the
Linux C branch or prove kernel behavior. The native result below supplies that
case-specific evidence. No parent task is closed by this fixture or source review.

## Native result for 9bb0fb60

[Run 41076338785](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41076338785)
passed on Linux x64 and arm64. Each report contains 36 rows, including four
actual mutation cases across the read/write combinations. All observe matching
ready/pinned TIDs, descriptor/address/path replacement, the original inode now
unlinked, successful syscall completion and private token/ack exchange, with no
host receipt. The exact missing-endpoint control instead records actual host
delivery, successful send and no private roundtrip after the same mutations.

Original reports and control logs are retained under
`scripts/probes/plugin-native-entry/evidence/linux-private-stream-mutation-9bb0fb60/{x64,arm64}`.
They bind tested merge `0aed910c2e00671ecf2dd3d88fb98b028fdba546` and source manifest
SHA-256 `a59ac3e0c8fed62905517f9ce468b13810f8f6d79955de4b902ee4f04db7b643`.
The manifest was independently recomputed from that exact tree, which was also
verified equal to branch head `9bb0fb6040af2b9f0a0e3767148a2c889afd9c73`.
These receipts do not establish later relative-path handling or full IPC support.
