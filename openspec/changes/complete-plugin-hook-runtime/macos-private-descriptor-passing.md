# macOS private descriptor passing and pinned owner effects

This extends the approved native IPC proof independently of the pending Linux
mediation decision. It does not activate production admission or complete the
remaining ancillary/network compatibility requirements.

Each owned-listener case now sends one dummy-file descriptor over a private
stream socketpair using SCM_RIGHTS. The sender closes its original descriptor
and checks EBADF before reception. The receiver requires the actual SCM_RIGHTS
header, one descriptor, matching regular-file device/inode identity, successful
read/write and a verified FD_CLOEXEC flag. Successful byte delivery alone cannot
stand in for descriptor reception.

The parent creates and pins the empty dummy file before launching the guest.
The guest writes `A`, transfers its independently opened descriptor, then reads
`A` and writes `B` through the received descriptor. It subsequently unlinks the
original pathname and replaces it with a symlink to another owned dummy file
containing `XX`. After guest settlement, the parent reads at most three bytes
from its retained descriptor, checks regular-file type and the two-byte limit,
requires link count zero and observes `AB`. It never reopens the guest-controlled
path. This avoids an unbounded or symlink-following owner read while confirming
the file effect belongs to the original owned object.

Two precise controls cover different failures:

- `--drop-rights` removes the receive control buffer only in confined cases.
  Its unconfined control still passes; the first confined case receives `T`,
  has closed its original sender descriptor, receives no descriptor and leaves
  the pinned file as `A`. It must fail `private SCM_RIGHTS exchange failed`.
  The local Darwin kernel does not set MSG_CTRUNC in this case. The result is
  recorded; absence of that flag is not used as evidence of descriptor reception.
- A temporary copied driver reopens the substituted pathname for its final
  file-effect observation. Genuine descriptor operations and path replacement
  succeed, but it reads `XX` and must fail `received descriptor file effect`.
  This directly detects regression from pinned ownership to path re-resolution.

The existing widened-network-scope control remains and must still reach the
owned host listener after all private operations succeed. Invalid startup or a
broken private exchange cannot satisfy these controls. The new header is in
both source manifests; runtime fixtures using other modes retain their original
behavior.

## Local result

On macOS arm64/Darwin 25.6.0, all nine descriptor/listener cases pass. All eight
capability combinations preserve private streams/aliases, correct peer identity,
SCM_RIGHTS transfer and the owner-observed `AB` effect on the unlinked original,
while delivering no host token. All three counterexamples fail their named
assertions after their required positive operations. The native fixture-built,
native packaged-helper and Node runtime regressions each pass nine cases: 27
runtime cases in addition to the nine extended native cases. Other runtime and
hosted Intel/arm64 checks remain pending for this revision.

Reports and control logs are retained under
`scripts/probes/plugin-confinement/evidence/macos-private-rights-local`.
The independently recomputed descriptor/listener manifest SHA-256 is
`76fa300cb4534e9d5e571d84b9560db6845eab015cefa10f02023721b4399c1c`;
the runtime manifest SHA-256 is
`dc8028c477b35faa33f4b99183bc2c6e2c8f54c81d58e0785e462eeecdd8e67f`.

Still required: concurrent endpoint mutation, datagram/seqpacket, ancillary
credential behavior and broader descriptor cases, proxy/network/secret ceilings,
complete native resource ownership and actual App/server execution. Parent
acceptance tasks remain open.
