# macOS concurrent datagram alias replacement

This extends the approved native proof with concurrent pathname mutation; it does
not activate production launch or resolve the pending Linux direction decision.

The native fixture binds a private scratch datagram server and creates a scratch
symlink to it. A positive exchange through that alias must complete before the
race starts. A second native thread atomically replaces the alias between the
private endpoint and the existing owned host endpoint. It creates a temporary
symlink and renames it over the alias; it never changes either target socket.

The sender alternates destination-bearing `sendto` and `sendmsg` for sixteen
attempts. Each attempt waits for a newly published successful replacement while
the mutator continues running. Private delivery receives a distinct `P`
acknowledgement from the actual private receiver; actual host delivery receives
`A` from the independent native owner. Only EACCES/EPERM qualifies as a policy
refusal. Missing acknowledgement, invalid payload, other syscall errors, absent
mutation progress or failed thread settlement fail the test.

Mutation is bounded to 4096 successful swaps, uses atomics and monotonic wait
limits, and is stopped and joined before any stack/path resources are released.
A join failure terminates the fixture rather than releasing resources under a
possibly live thread. After settlement, a fixed host-target alias exchange
requires delivery when unconfined and refusal when confined. This gives both
positive endpoint controls independently of scheduling during the race.

The owner records all host packets. The unconfined total must equal six original
routes, one fixed host control and the race's acknowledged host packets. All
eight confined cases require zero actual host receipts and no host race
acknowledgements, while retaining every existing private stream, descriptor and
datagram check. The widened-scope control must expose actual stream and datagram
host delivery and fail the named datagram isolation assertion. It validates the
receipt total against the observed race acknowledgements rather than assuming a
fixed race schedule.

## Local evidence

On arm64/Darwin 25.6.0, all nine IPC cases pass. The retained unconfined run has
eight private and eight host acknowledgements during sixteen race sends, with
fifteen actual host packets including the fixed controls. Each confined case has
eight private acknowledgements, eight policy refusals and zero host receipts.
These counts describe this run; tests do not require a particular race schedule.

Native fixture, native packaged-helper and Node fixture runtime regressions each
pass nine rows, with no unsupported rows. Widened-scope, dropped-rights and
path-reader counterexamples fail their exact assertions. Original reports and
logs are in `scripts/probes/plugin-confinement/evidence/macos-datagram-race-local`.
Independently recomputed source hashes are:

- IPC: `7eae036bc0756198bd522e570f6e2c7a357efab0f0eb543b7ced11d4e4f88cce`.
- Runtime: `05c2c5808004a944c2582857a16df3f4a2266abbe5911d68886c36aeebd3b5ad`.

Hosted verification is pending. This tests one concurrently replaced symlink
with destination-bearing sends. It does not establish every pathname/FD/address
mutation, seqpacket or ancillary credential behavior, proxy/network/secret
ceilings, descendant settlement or App integration. Parent acceptance stays open.
