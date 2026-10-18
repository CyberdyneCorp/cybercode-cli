## ADDED Requirements

### Requirement: Peer listener
(P2) When user config sets `peers.listen`, the server SHALL open a second listener that serves only `/api/v1/peer/*` and answers `404` for every other path; the main API SHALL keep its own bind. A non-loopback peer listener SHALL use TLS with a self-signed ECDSA P-256 certificate the server generates on first use and keeps in `<state>/peer-tls/` (mode 0600); `tls: false` SHALL be refused unless the address is loopback. The fingerprint SHALL be the SHA-256 of the certificate's SubjectPublicKeyInfo and SHALL be recorded in `server.json`. Requests carrying an `Origin` header SHALL be refused with `403`. `peers` SHALL be accepted only from user config; project and local config setting it SHALL fail validation.

#### Scenario: Only peer routes are served
- **WHEN** a client with a valid peer token requests `GET /api/v1/sessions` on the peer listener
- **THEN** the response is `404`

#### Scenario: Plain listener off loopback
- **WHEN** user config sets `peers.listen: { address: "0.0.0.0:4748", tls: false }`
- **THEN** the server refuses to start the peer listener and logs why, and the main API still starts

#### Scenario: Browser request
- **WHEN** a request to the peer listener has `Origin: http://192.168.1.20:8080`
- **THEN** the response is `403`

### Requirement: Peer discovery
(P2) With `peers.mdns: true` (default `false`) and a non-loopback peer listener, the server SHALL publish `_cyber._tcp` named `cyber-<machine>` with TXT `role=peer`, `v=1`, `machine` and `fp`, and SHALL unpublish it on shutdown. `cyber peers discover [--timeout <duration>]` SHALL list discovered servers with machine, address, fingerprint and whether each is already a remote. Discovery SHALL NOT add a peer or grant trust; `cyber peers add` SHALL fail when a discovered fingerprint for the same address differs from the pairing code's.

#### Scenario: Discover a desktop
- **WHEN** `desk` publishes its peer listener and `laptop` runs `cyber peers discover`
- **THEN** `desk` is listed with its address and fingerprint and `not added`
