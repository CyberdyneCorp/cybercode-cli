## ADDED Requirements

### Requirement: Direct peer transport
(P2) The system SHALL read `peers` from config: a list of `{ name, url, auth: { type: "password", password } | { type: "mtls", cert, key } , directory_map? }` naming other `cyber` servers the user controls (another machine on the LAN, a VPN host, an SSH-forwarded port). `cyber peers list|add|rm|test <name>` SHALL manage them, and `cyber peers discover` SHALL list `_cyber._tcp` servers found by mDNS for one-step adding. `list_sessions` SHALL include peer Sessions with `kind: "peer"` and address `<name>@<peer>`; `send_message`, `watch_session` and `cyber attach <session>@<peer>` SHALL work over the peer's public API without a Cyber Account or Relay. Peer senders SHALL be treated as off-machine: inbound default `hold`, `message.send ask`, and `remote.attach ask`. A peer URL that is not loopback SHALL require `https` unless `insecure: true` is set explicitly.

#### Scenario: Two laptops on one network
- **WHEN** laptop A lists `{ name: "desk", url: "https://desk.local:4747", auth: { type: "password", password: "{env:DESK_PASSWORD}" } }` in `peers` and a Session on `desk` is running
- **THEN** `list_sessions` on A shows it with `kind: "peer"`, and `send_message({ to: "migration@desk", text })` is held on `desk` until its user releases it

#### Scenario: Peer offline
- **WHEN** a configured peer does not answer within 5 seconds
- **THEN** its Sessions are listed with `state: "offline"` and `canReceive: false`, and a send fails locally with `PeerUnreachableError`
