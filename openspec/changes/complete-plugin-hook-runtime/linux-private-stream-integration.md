# Private pathname stream integration in the broker candidate

This implements the first real broker use of the
[endpoint identity primitive](linux-unix-endpoint-identity.md), under the approved
[integration direction](native-host-integration.md). It is an isolated candidate
increment, not completed private IPC or production activation. Datagram,
seqpacket and full runtime/owner acceptance remain required.

The copied helper's explicit `U` profile record selects one fresh, canonical,
protected scratch directory bound as its own mount. Only explicitly owner-created
proxy endpoints may be present before guest startup. The caller must establish
that origin and protect the profile/root ancestry; the candidate's bounded
parser, no-follow directory open and pinned descriptor do not supply admission.
The guest never inherits the trusted root or endpoint descriptors.

For this phase the family filter permits new UNIX stream sockets and stream
pairs, while retaining the current IP families. UNIX datagram/seqpacket creation
remains denied pending destination-bearing send mediation. Native bind/listen
operate in the private network namespace. An additional Landlock layer restricts
pathname socket creation to scratch, preserving the original bound pathname
instead of replacing it with a broker-internal path.

That layer explicitly handles and grants `REFER` at the root so it does not
silently remove ordinary file moves permitted by the outer filesystem policy.
All existing filesystem restrictions still intersect it. Fresh scratch is its own
mount, so writable project sockets cannot be hard-linked into it; this origin
precondition is essential, including after a source pathname is unlinked.

Connect notifications still pin the exact requesting thread's socket. For an
absolute scratch pathname the broker uses beneath/no-symlink/no-magic-link
resolution and checks the actual endpoint inode, retaining its O_PATH descriptor
through the worker operation. The worker keeps only socket, result pipe and
endpoint handles at fixed slots, then connects through its own procfs reference.
It never continues a mutable inspected pathname or re-resolves a guest descriptor.
Private abstract stream destinations rely on the mandatory private net namespace.

The existing native matrix adds a distinct private-stream mode: 16 host endpoint
denials and 16 pair cases must retain read isolation, private stream/socketpair
communication and private IP/thread-local exchanges. Ordinary regular-file
reparenting must match `fs.write`, without requiring content reads. Writable
public-socket cases must independently observe EXDEV when attempting a hard link
into scratch. The old broker, cancellation, mutation, descendant and capacity
matrices remain and must rerun because worker handle ownership changed.

Two precise temporary controls omit either the socket-creation layer or its
REFER compatibility grant. The first must bind a fixture socket in the writable
public Location and fail the named private-creation assertion. The second must
fail the named ordinary-file-move assertion in a writable Location. Neither an
unrelated setup failure nor blanket refusal counts as either counterexample.
The runtime workflow adds the five-family startup/capability matrix for this
helper; those existing rows do not themselves prove runtime pathname IPC.

Local checks pass Rust example Clippy with warnings denied, portable fixture C
syntax (excluding its Linux-only branches), TypeScript/Python syntax, workflow
shell/embedded Python, formatting/diff and all 16 temporary builder transformations
with compiler interception. The native SDK emits two Seatbelt deprecation
warnings. The hosted result below supplies Linux compilation/kernel evidence for
the exact tested source, rather than assigning local macOS checks that scope.

## Native result for 2b86ac8b

[Run 41058437042](https://github.com/CyberdyneCorp/cybercode-cli/actions/runs/41058437042)
passed on Linux x64 and arm64. Each architecture's private-stream report contains
32 cases: 16 ambient endpoint denials and 16 pair cases with successful scratch
stream/IP/thread-local exchanges, independent read/write expectations and regular
file moves matching `fs.write`. Both writable public-socket cases observe EXDEV
for the cross-mount alias attempt. The exact missing-MAKE_SOCK and missing-REFER
controls fail their named assertions after positive startup and scratch exchange.

Each architecture's five private-stream runtime reports contain nine cases,
including an unconfined control, with no unsupported rows: 45 cases per
architecture. These are startup/capability regressions, not full runtime IPC
acceptance. The workflow also passes its existing broker cancellation, mutation,
descendant and worker-capacity checks after the worker descriptor changes.

Original reports, both control logs and the five runtime JSON reports are retained
under `scripts/probes/plugin-native-entry/evidence/linux-private-stream-2b86ac8b/{x64,arm64}`.
They bind original checkout `2b86ac8b8ea70bb2c5030ca6b22e948b345e3f7b`, kernel
`7.0.0-1017-azure`, socket source manifest SHA-256
`1f096940494d0a1bb9756ff4c020fc7fd75a591f377557b1cd791c8d837f9b59`
and runtime source manifest SHA-256
`52bcf7e1df59d36b59b85de1181c2e85d7dfcc79bfa3cf728c06e65b75831630`.
Both manifests were independently recomputed from that exact Git tree. Every
runtime report's input/executed helper digest matches its architecture's socket
report. The tested commits remain on `evidence/plugin-native-host-pre-76-merge`
when the implementation branch is rebased onto the #76 squash merge. These
original receipts are not relabeled as execution of the rebased head.

The [real broker pathname replacement case](linux-private-stream-mutation.md)
now tests the post-pin race; its new hosted evidence is pending.

Still required before complete IPC/production acceptance: verified post-pin
replacement, relative/alias forms, datagram/seqpacket and
destination-bearing sends, ancillary FD/credential and peer-identity behavior,
exact proxy endpoint ownership and network ceilings, full runtime IPC controls,
immutable admission, generation quarantine and complete resource settlement.
Task N2 and parent tasks 2.1–2.4 remain open. No ordinary tool or production helper
policy is changed by these copied-source experiments.
