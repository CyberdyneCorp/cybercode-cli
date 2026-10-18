## Why

Users run Cyber Code on more than one machine (a laptop and a desktop, a laptop and a build box on a VPN) and want Sessions on one to message Sessions on the other. `implement-cross-session-messaging` ships the local transport only and defers the P2 requirement "Direct peer transport" to this change (its design, Resolved Decision 1). It leaves the extension points this change fills: the `@` address tier (today `name@machine` fails with `AccountRequiredError`), the cross-machine `hold` default, the `message.send ask` default for `@` targets, and `origin_kind` as an open set.

The P2 requirement as written is not safe to implement literally. It authenticates peers with `{ type: "password" }`, and the only password the server has is the local one of "Local password authentication", which grants full control of every Session and every tool on that machine. Giving machine A the password of machine B so A can send messages would let anything on A that reads the config run commands on B. The requirement also assumes the full `/api/v1` API is reachable off-loopback, which means binding the whole API to the network.

## What Changes

- **A separate peer listener.** `peers.listen` (off by default) opens a second listener that serves only the new `/api/v1/peer/*` route group: hello, Session listing, message send and status, Session status, and (opt-in) attach. The main API keeps binding `127.0.0.1`. Non-loopback peer listeners are HTTPS only, with a self-signed certificate the server generates and keeps.
- **Scoped peer grants instead of passwords.** `cyber peers invite <name>` on the receiving machine creates a grant (scopes `message`, optionally `attach`), stores only a hash of its 256-bit token, and prints a one-line pairing code that carries the URL, the certificate fingerprint and the token. `cyber peers add <name> <code>` on the sending machine stores the token in the OS keyring and pins the fingerprint. The server authenticates the client by the token; the client authenticates the server by the pinned fingerprint. `cyber peers revoke <name>` ends a grant immediately.
- **Peer identity is assigned, not claimed.** The receiving server names a peer sender after the grant (`ses_…@laptop`, `review@laptop`), so inbound rules, rate limits and audit records cannot be spoofed by the sender.
- **Messaging over peers.** `list_sessions` fans out to configured peers (5-second timeout, offline entries); `send_message` and `watch_session` resolve `name@peer`; the target runs its normal pipeline with a peer sender (inbound default `hold`); the sender gets the outcome synchronously and polls held messages for their release or drop, emitting `messaging.outcome` events; watches poll the peer's Session status. Unreachable peers fail with `PeerUnreachableError`; there is no store-and-forward.
- **Attach over peers.** `cyber attach <session>@<peer>` opens the TUI on one remote Session through the peer listener, only with an `attach` grant and after the target's `remote.attach` permission (default `ask`).
- **Discovery.** With `peers.mdns: true` the peer listener is published as `_cyber._tcp` with its fingerprint; `cyber peers discover` lists what it finds. Discovery never grants trust: adding a peer still needs a pairing code.
- **CLI.** `cyber peers list|add|rm|test|discover|invite|revoke`.

Out of scope: the Relay and Cyber Account transport (P3 "Cross-machine transport"), NAT traversal, store-and-forward queues, peers between different OS users or accounts, attachments in messages, and distributed agent-teams teammates (P3 in `agent-teams`; they will reuse this peer path).

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `cross-session-messaging`: "Direct peer transport" rewritten (peer listener, grants, pinned TLS, assigned identity, asynchronous outcomes; `password`, `mtls`, `insecure` and `directory_map` removed); added "Peer pairing and grants", "Peer outcome tracking", "Peer watches and listing" and "Peer attach".
- `server-api`: added "Peer listener" and "Peer discovery".
- `cli-commands`: "Attach command" (`<session>@<peer>`) and "Resource command groups" (`peers` gains `invite` and `revoke`); added "Peers command".

## Impact

- Code: `crates/cyber-server/src/http/peer/` (new route group, grant guard, TLS listener), `crates/cyber-server/src/runtime/messaging/` (`peer.rs` client, outcome tracker, peer sender kind, address tier), `crates/cyber-store/migrations` (`peer_grant`, remote outcome columns on `xsession_message`, remote watches), `crates/cyber-core/src/config/` (`peers`), `crates/cyber-cli/src/commands/peers.rs` (new) and `attach`, `crates/cyber-tui` (peer badges, held cards naming the machine), `sdk/openapi.json` and `sdk/typescript`.
- Dependencies: `rcgen` (certificate generation) and `mdns-sd` (discovery) are new; `tokio-rustls`, `rustls` and `keyring` are already in the lock file.
- Docs: README (cross-machine messaging section), `docs/features.md`, `docs/implementation/p2-status.md`.
- Sequencing: implement and archive after `implement-cross-session-messaging`, which owns the pipeline, records and tools this change extends.
