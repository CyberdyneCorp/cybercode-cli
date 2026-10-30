# macOS private runtime UNIX exchanges

This is independent progress within the approved native-proof scope. The
[Linux mediation revision](linux-kernel-unix-revision.md) remains unapproved;
this extension does not select that direction or activate production hosts.

The existing pre-runtime macOS signing candidate checks capabilities without
exchanging data through runtime-owned UNIX pathname sockets. Its default-deny
network policy also blocks private scratch socket creation/communication. The
opt-in `--macos-private-unix` extension adds scratch-scoped network-inbound/local
and network-outbound/remote UNIX socket rules to the experimental Seatbelt
profile. It requires the pre-runtime signing candidate. Ordinary profiles and
production launch paths do not change.

The same untrusted Node/Python/Bun/Deno/native fixtures used by the Linux stream
candidate now require real absolute and relative pathname token/ack exchanges on
macOS. Every capability combination and the unconfined control must report
`private_unix: [true, true]`. Original read/write/shell, pre-main, descriptor,
constructor and image-signature assertions remain. Reports identify the macOS
candidate separately from the Linux broker candidate.

Darwin's bounded UNIX address cannot contain the per-user TMPDIR plus long case
names. For this opt-in experiment only, create the same private 0700 temporary
root beneath canonical `/tmp`. Keep the absolute socket exchange and its strict
expectation; do not replace it with a relative-only test or count an address-size
failure as policy enforcement.

The native signing workflow on both macOS architectures also runs a copied
fixture driver with only the scratch UNIX grants omitted. The unconfined row
must pass, then the no-capabilities row must fail specifically at
`private UNIX runtime exchange failed: [false,false]`. Generic profile syntax or
startup failure cannot satisfy this control. The packaged-helper workflow adds
the positive exchanges to all five runtime families and system Python.

## Local result

On macOS arm64, Darwin 25.6.0, native C, Node 22.20.0 and system Python 3.9.6 each
pass all nine pre-runtime rows using both the fixture-built launcher and the
freshly built packaged helper: 54 cases total, with no unsupported rows. Both
private exchanges succeed in every row, and the missing-grant control fails its
exact named assertion after the unconfined control succeeds. The default native
signing matrix also passes its nine rows without the opt-in observation field.
Original reports and the control log are retained under
`scripts/probes/plugin-confinement/evidence/macos-private-unix-local`.
Their independently recomputed source manifest SHA-256 is
`819f7e4455d5d9c4803197328ff0bef845803515202711349897af9260789ff7`.
Packaged reports record input helper SHA-256
`64ee74d21d1ddc470989450014f1e89fa33ab5aec230cd998890df3ec5717225`.
Each private launcher copy receives its fresh signing identifier, so its executed
digest is separately recorded rather than asserted equal to the input digest.

Bun/Deno are unavailable locally. Hosted Intel/arm64 runtime matrices and the
complete packaged-helper matrix are configured but pending for this revision. These stream
exchanges are not full private IPC acceptance: owned-host/alias adversarial
coverage, datagram/seqpacket, ancillary credentials/FDs, proxy/network ceilings
and complete production admission/settlement remain required. Native support
and parent task checkboxes stay open.
