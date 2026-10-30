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
warnings. Linux C compilation, kernel behavior and the new controls require
fresh hosted x64/arm64 results; no such result is claimed yet.

Still required before complete IPC/production acceptance: actual post-pin
pathname replacement in this broker, relative/alias forms, datagram/seqpacket and
destination-bearing sends, ancillary FD/credential and peer-identity behavior,
exact proxy endpoint ownership and network ceilings, full runtime IPC controls,
immutable admission, generation quarantine and complete resource settlement.
Task N2 and parent tasks 2.1–2.4 remain open. No ordinary tool or production helper
policy is changed by these copied-source experiments.
