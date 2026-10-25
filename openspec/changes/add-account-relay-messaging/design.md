## Context

Cross-session messaging has three transports today:
- **In-process:** on one server.
- **Direct peers:** own machines, paired with a code, reached over a peer listener with a pinned TLS certificate.
- **The project hub:** experimental, self-hosted and OIDC-backed.

The specs reserve a fourth for P3: the Relay, authenticated with a Cyber Account. `remote-control` already defines the Relay's outbound WSS connection, its self-hosted form and its end-to-end encryption between a Device and a server. This design reuses those pieces for machine-to-machine messaging and adds what is missing: machine identity, a roster, trust approval, team addressing and the connection's lifecycle.

Goal: a user signed in on several machines, on any networks, can see and message their own agents without pairing codes or open ports. Members of an org can do the same with each other's agents when every side opts in. The Relay must not be able to read messages or Session names, and must not be able to impersonate a machine.

## Decisions

### 1. Machine identity

On first use of the Relay, each server creates an X25519 key pair in `<state>/machine/key` (mode 0600) and a machine ID with prefix `mch_` (a ULID). The machine name comes from `messaging.relay.machine`, else the host name, limited to `[a-z0-9-]{1,32}` after normalization. `cyber machines rename <name>` changes it and tells approved machines over the encrypted channel. Names are unique within an account: on a clash, the Relay refuses the announce with `MachineNameTakenError` and the CLI suggests a rename.

The key is separate from the peer-listener TLS key, so rotating one does not break the other.

### 2. Connection lifecycle

`messaging.relay.enabled: true`, accepted only from user config or managed policy, together with a Cyber Account session, starts one outbound WSS connection to `messaging.relay.url`. `policy.remote.relay_url` overrides that URL, as with remote control. The connection authenticates with an access token carrying `cyber:relay` and `cyber:messaging` and announces `{ machine_id, public_key, protocol }`.

Behaviour:
- **Keepalive:** a ping every 20 s.
- **Reconnect:** with jittered backoff, from 1 s up to 5 min.
- **Logout:** closes the connection.
- **Token refresh:** reuses the account's serialized refresh.

The same socket will later also carry remote control, but enabling messaging exposes no Session to Devices. `cyber doctor` reports the Relay state, the machine ID, the key fingerprint and the counts of approved and pending machines.

### 3. Roster

For each account the Relay stores `{ machine_id, public_key, online, last_seen_ms }` and nothing about Sessions. Each machine subscribes to its account's roster. A teammate's machines appear only when:
- the org policy `policy.messaging.org_members` is true (read from the token's org claims and policy);
- the teammate set `messaging.relay.share_with_org: true`.

Then they appear under the member's verified `preferred_username`, or their `sub` when no username is set.

The Relay can see who is online and when. It cannot see names, Session titles or text.

### 4. Trust approval

A machine in the roster is `pending` until the user approves it locally. Approval is per pair of machines and is kept in a `machine_trust` table: `machine_id`, `public_key`, account `sub`, `state` (`pending`, `approved`, `rejected`), `approved_at` and `name`.

- **The code.** Both sides show the same six-word code: BIP-39 words from `SHA-256(sorted(pkA, pkB))`. The user compares the codes and runs `cyber machines approve <name|mch_id>`, or presses `a` on the pending card in the TUI, on each side.
- **One side is not enough.** If only one side approves, no message flows.
- **A key change resets trust.** If a known `machine_id` shows up with a new public key, it becomes `pending` again, with a warning naming the old fingerprint.
- **Why approval is required.** Trusting the roster alone would let anyone controlling the Relay, or holding a stolen account token, add a machine and read held-message metadata. Approval is a one-time step for each pair of machines.
- **Revocation.** `cyber machines forget` deletes the trust record and closes channels at once. `reject` keeps a tombstone so the machine stops prompting.

### 5. Encrypted channel

After approval the two machines open an end-to-end encrypted channel through the Relay. It uses the handshake and frame format of `remote-control` "End-to-end encryption":
- long-term machine keys;
- an ephemeral X25519 key per connection;
- HKDF-SHA256 key derivation;
- ChaCha20-Poly1305 frames with a monotonic nonce;
- after 3 replayed nonces, the channel closes.

The Relay sees only `{ from mch_, to mch_, size, time }`. Channel messages reuse the peer route shapes (hello, sessions, send, status, outcome, watch-status) as typed frames, so the messaging runtime treats a Relay machine like a peer behind a different transport.

### 6. Addresses

New address tiers resolve after explicit peer remotes:
- `<session>@<machine>` is an approved machine of the caller's own account.
- `<session>@<member>/<machine>` is an approved machine of an org member.

The rules:
- **Clash with a peer name.** If a configured peer and an approved own machine share a name, the bare form fails with `AmbiguousAddressError` listing `peer:<name>` and `machine:<name>`. `cyber machines rename` or `cyber peers rm` resolves it.
- **Sender identity is assigned.** The receiver names the sender from the channel, never from frame fields: `<session>@<machine>` for its own account, `<session>@<member>/<machine>` for a teammate.

### 7. Delivery and outcomes

A send to a Relay machine works as follows:
1. It travels as an encrypted `send` frame.
2. The target runs its normal pipeline: inbound rules, limits, chain hops, records.
3. The target answers `delivered`, `held` or `refused`.
4. Held messages report `released` or `dropped` later through the same outcome tracker as direct peers, which polls every 5 s, backs off to 5 min, and expires after 7 days.

Default inbound rules:
- **Own account:** `hold`. A `deliver` pattern naming the machine (`*@laptop`) delivers.
- **Another account:** always `hold`. No `deliver` pattern and no Session override can bypass it. Release rechecks that the trust record is still approved and that the org policy still allows it.

There is no store-and-forward. If the target is offline or does not answer within 5 s, the send fails with `MachineUnreachableError { machine }` and records `failed / unreachable`.

### 8. Listing and watches

`list_sessions` asks each online approved machine with a 5 s timeout and a 15 s cache, in parallel with peers. Each machine returns at most 50 root Sessions that would accept the caller's messages. Locations show the workspace name only, unless the receiver sets `peers.share_paths` (the same switch as peers). Offline machines appear as one `*@<machine>` entry with state `offline`. `watch_session` on a Relay address polls the status frame every 15 s and expires after 12 hours.

### 9. Self-hosted Relay

`cyber relay serve --listen <addr> --issuer <discovery url> [--no-entitlement-check]` implements:
- the roster;
- the routing for this change;
- token verification against the issuer, including introspection of org claims for team rosters.

Its limits:
- **Rate:** 120 frames per minute per machine.
- **Frame size:** 64 KiB.
- **Connections:** 32 per account.

The Relay serves no web client in this change.

### 10. Surfaces

- **CLI:** `cyber machines list` shows each machine's name, ID, account or member, trust state, online state and fingerprint. `whoami` shows this machine's name, ID and fingerprint. `rename`, `approve`, `reject` and `forget` act on another machine.
- **TUI:** a footer badge `⚇ <n> pending` and a pending-machine card with the code and `a` to approve or `x` to reject. Relay entries carry machine badges in the agent directory (separate P1 change).
- **API:** `GET /api/v1/machines`, `POST /api/v1/machines/{id}/approve|reject`, `DELETE /api/v1/machines/{id}` and `PATCH /api/v1/machines/self` (rename).
- **SDK:** matching methods.

### 11. Security defaults (summary)

- **Off by default.** Turning it on takes user config only; projects cannot.
- **Outbound only:** no inbound ports.
- **End-to-end encryption:** the Relay sees routing metadata only.
- **Mutual approval** for every pair of machines; a key change resets trust.
- **Cross-account messages are always held**, behind org policy and member opt-in.
- **Messages stay untrusted text** with no authority. `message.send` still asks for every `@` target.
- **The managed policy can turn it off:** `policy.messaging.relay: false`.

## Risks / Trade-offs

- **Metadata exposure:** the Relay learns machine counts, online times and traffic volume. This is documented, and self-hosting is the mitigation.
- **Approval friction:** approving each pair on both sides is extra work, but it is one-time and cheaper than a pairing code because no address or code is copied. An option to trust the account roster alone was considered and rejected for the hosted Relay, because it makes the Relay a trusted party.
- **No offline queue:** a sleeping laptop cannot receive messages. Callers get a clear error and can use `watch_session` on the `*@machine` entry once the machine comes back online. A queue of encrypted messages on the Relay is an open question.

## Migration Plan

This is additive. Existing direct peers keep working and keep priority for their names. Machines with `messaging.relay.enabled` unset behave exactly as before.

## Open Questions

1. Should the Relay keep an encrypted offline queue (for example, 7 days and 100 messages per target machine), so a sleeping laptop receives messages when it wakes? It is left out of this change for a smaller attack surface.
2. Team member naming: `preferred_username` can change. Should addresses use an immutable org member handle once CyberdyneAuth exposes one?
3. Should approval of a machine of the same account be skippable on a self-hosted Relay the operator fully trusts (`messaging.relay.trust: "roster"`)?
