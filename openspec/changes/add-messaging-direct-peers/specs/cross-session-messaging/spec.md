## MODIFIED Requirements

### Requirement: Direct peer transport
(P2) The system SHALL connect `cyber` servers the same person controls (another machine on the LAN, a VPN host, an SSH-forwarded port) through a peer listener and scoped peer grants, without a Cyber Account or Relay. `peers.remotes` in user config SHALL list `{ name, url, fingerprint, enabled? }`, with each peer's token stored in the OS keyring or given as `{env:NAME}`. A remote `url` that is not loopback SHALL require `https`, and the client SHALL accept only the server certificate whose SubjectPublicKeyInfo SHA-256 equals the pinned `fingerprint`. An address `X@P` SHALL resolve `P` against `peers.remotes` names and fail with `PeerNotConfiguredError` when none matches. `list_sessions` SHALL include peer Sessions with `kind: "peer"` and address `<name>@<peer>`, and `send_message` and `watch_session` SHALL work on them. The receiving server SHALL name the sender after the grant that authenticated the request (`ses_…@<grant>`, `<title>@<grant>`), never after names in the request body. Peer senders SHALL be treated as off-machine: inbound default `hold`, `message.send ask`, and `remote.attach ask`. The size limit, rate limits, chain hops and recursion depth SHALL apply as for local messages, and both Sessions SHALL record the message with `origin_kind: "peer"`. A peer that does not answer within 5 seconds SHALL be listed offline and a send to it SHALL fail with `PeerUnreachableError`; messages SHALL NOT be queued for later delivery.

#### Scenario: Two laptops on one network
- **WHEN** laptop A has a remote `{ name: "desk", url: "https://desk.local:4748", fingerprint: "sha256:9f2c…" }` paired with a grant on `desk`, and a Session named `migration` runs on `desk`
- **THEN** `list_sessions` on A shows `migration@desk` with `kind: "peer"`, and `send_message({ to: "migration@desk", text })` is held on `desk` until its user releases it

#### Scenario: Peer offline
- **WHEN** a configured peer does not answer within 5 seconds
- **THEN** its Sessions are listed with `state: "offline"` and `canReceive: false`, and a send fails locally with `PeerUnreachableError`

#### Scenario: Certificate does not match the pin
- **WHEN** a server at the peer URL presents a certificate whose fingerprint differs from the pinned one
- **THEN** no request body is sent, the send fails with `PeerAuthError` reason `fingerprint_mismatch`, and the remote is shown `auth_failed`

#### Scenario: Sender cannot claim another machine
- **WHEN** a request authenticated by grant `laptop` sends `from_name: "build"` to a Session whose inbound rules `refuse` `*@workstation` and `deliver` nothing else
- **THEN** the sender address is `build@laptop`, the default `hold` applies, and the record names `laptop`

#### Scenario: Unknown peer name
- **WHEN** a Session sends to `api@nas` and no remote is named `nas`
- **THEN** the send fails with `PeerNotConfiguredError { peer: "nas" }` before any network request

## ADDED Requirements

### Requirement: Peer pairing and grants
(P2) `cyber peers invite <name> [--scope message|attach]... [--expires <duration>]` SHALL create a grant on the receiving machine with a 256-bit random token, store only its SHA-256 hash, and print once a pairing code carrying the peer URL, the machine name, the certificate fingerprint and the token. The default scope SHALL be `message`. `cyber peers add <name> <code>` SHALL store the token in the OS keyring (falling back to a 0600 file with a warning), pin the fingerprint, write the remote to user config and test it. Peer requests SHALL authenticate with `Authorization: Bearer cyp_…`; the peer listener SHALL accept no other credential, and the main API SHALL NOT accept `cyp_` tokens. Unknown, revoked or expired tokens SHALL receive `401`; a token without the route's scope SHALL receive `403` `PeerScopeError`. `cyber peers revoke <name>` SHALL end a grant for the next request. Each grant SHALL be limited to 120 requests per minute.

#### Scenario: Pair two machines
- **WHEN** the user runs `cyber peers invite laptop` on `desk` and `cyber peers add desk '<code>'` on `laptop`
- **THEN** `cyber peers test desk` on `laptop` prints `desk`, its version and scopes `message`, and `desk`'s database holds the token hash but not the token

#### Scenario: Revoked grant
- **WHEN** `cyber peers revoke laptop` runs on `desk` and `laptop` then sends a message
- **THEN** the request receives `401` and the send fails with `PeerAuthError` reason `unauthorized`

#### Scenario: Message grant cannot attach
- **WHEN** a grant with scope `message` requests an attach route
- **THEN** the response is `403` `PeerScopeError`

### Requirement: Peer outcome tracking
(P2) The sending server SHALL record the synchronous outcome of a peer send and SHALL poll the peer for messages held there, with backoff from 5 seconds to 5 minutes, for up to 7 days, after which the outcome SHALL become `expired`. A change SHALL emit the live `messaging.outcome` event and the durable `messaging.message.released.1` or `messaging.message.dropped.1` in the sender Session. Polling SHALL resume after a server restart. `POST /api/v1/peer/messages` SHALL be idempotent on the grant and message ID.

#### Scenario: Release seen by the sender
- **WHEN** a message to `migration@desk` is held on `desk` and released there an hour later
- **THEN** the sender Session receives `messaging.message.released.1` within the next poll interval, including after a restart of the sending server in between

#### Scenario: Retry after a timeout
- **WHEN** the sender retries a send with the same `msgx_` ID after a timeout
- **THEN** the target returns the first outcome and stores one message

### Requirement: Peer watches and listing
(P2) `list_sessions` SHALL query enabled peers in parallel with a 5-second timeout, cache their entries for 15 seconds, return at most 50 entries per peer, and show an unreachable peer as one offline entry. A peer SHALL list only root Sessions the requesting grant could message, with only the workspace name as location unless `peers.share_paths` is `true`. `watch_session` on a peer Session SHALL poll its status every 15 seconds and fire when it goes from `running` to `idle`, with the 12-hour expiry of local watches.

#### Scenario: Watch a remote Session
- **WHEN** a Session watches `migration@desk` while it runs and it becomes idle
- **THEN** the watcher receives the idle notice within 15 seconds of the change

#### Scenario: Refused grant hides Sessions
- **WHEN** a Session on `desk` refuses `*@laptop`
- **THEN** it does not appear in `laptop`'s `list_sessions`

### Requirement: Peer attach
(P2) `cyber attach <session>@<peer>` SHALL open the TUI on one Session of a peer through the peer listener, only with a grant that has scope `attach`. Each attach SHALL raise the `remote.attach` permission in the target Session with resource `peer:<grant>` (default `ask`), wait up to 2 minutes, and fail with `PermissionDeniedError` when refused or unanswered. The attached client SHALL reach only that Session's event stream, prompt, interrupt and permission-reply routes.

#### Scenario: Attach approved
- **WHEN** `laptop` runs `cyber attach migration@desk` with an `attach` grant and the user on `desk` approves the request
- **THEN** the TUI on `laptop` shows the Session's transcript and can prompt it

#### Scenario: Attach without approval
- **WHEN** nobody answers the `remote.attach` request within 2 minutes
- **THEN** the attach fails with `PermissionDeniedError` and no Session data is sent
