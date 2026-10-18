## Context

`implement-cross-session-messaging` builds one pipeline (`Messaging::send`) used by the model tools, HTTP, the CLI and MCP serve. Senders are `Session(ses_)` or `Client(label)`; `origin_kind` is an open set; any address with `@` fails with `AccountRequiredError`; inbound evaluation already has a `hold` default for cross-machine senders with no producer. This change adds the producer: a direct HTTPS link between two `cyber` servers the same person controls.

What exists to build on:
- The server binds `127.0.0.1` and authenticates every TCP route with HTTP Basic and the local password (`http/guard.rs` `authorized`), or the Unix socket by peer credentials. That password is full control of the machine.
- `guard.rs` already has `loopback_peer` / `loopback_host` checks (used by MCP serve).
- `cyber serve --mdns` exists as a flag but only warns; nothing is published.
- `rustls`, `tokio-rustls` and `keyring` are in `Cargo.lock` (client TLS and the import keyring path).

**Goals:** two machines message each other's Sessions with the P1 semantics unchanged (limits, inbound rules, hops, depth, records in both Sessions); nothing on one machine gains more power on the other than "put a message in an inbox, which is held by default"; setup is two commands; no account, no Relay, no CA.

**Non-Goals:** the Relay and account transport (P3); NAT traversal or hole punching (users reach each other on a LAN, a VPN such as Tailscale or WireGuard, or an SSH-forwarded port); store-and-forward when a peer is offline; peers belonging to different people (no org policy; each grant is one person's two machines); attachments; distributed agent-teams teammates (P3 in `agent-teams`, which will resolve teammates on other machines through this peer client); `directory_map` from the old requirement (no feature needs path translation once attach goes through the peer's own Session routes).

## Decisions

### 1. Transport: a separate, minimal peer listener

`peers.listen` opens a second listener. It serves only `/api/v1/peer/*`; every other path returns `404`. The main API never leaves loopback.

Alternatives considered:
- **Bind the main API off-loopback** (`cyber serve --hostname 0.0.0.0`, as the old requirement implied). Rejected: the whole API (prompts, shell, files) becomes network-reachable behind one password.
- **mTLS with client certificates.** Rejected as the default: it needs a CA or per-pair certificate exchange in both directions, adds a step to every pairing, and gives no more protection than a pinned server certificate plus a 256-bit bearer token over that TLS session.
- **SSH tunnels only.** Kept as a supported setup, not the only one: requires SSH access and a long-running tunnel, which many laptop-to-desktop setups lack. A peer listener bound to loopback (`tls: false` is allowed only there) behind `ssh -L` works with the same grants.

Config (user scope only; project config may not set `peers`, as it could otherwise open a listener):

```jsonc
"peers": {
  "listen": { "address": "0.0.0.0:4748", "tls": true },  // absent = no listener
  "mdns": false,
  "share_paths": false,                                    // listings show only the workspace name
  "machine": "desk",                                       // name shown to peers; default: hostname
  "remotes": [
    { "name": "laptop", "url": "https://laptop.local:4748",
      "fingerprint": "sha256:9f2c…", "enabled": true }      // token lives in the keyring
  ]
}
```

Rules: `tls: false` is refused unless `address` is loopback; a `remotes[].url` that is not loopback must be `https`; there is no `insecure` switch. The listener starts and stops with the server and `cyber service restart`; `server.json` records `peer_url` and the fingerprint.

TLS identity: on first start with a listener, the server generates an ECDSA P-256 key and a self-signed certificate (`rcgen`, SANs: `machine`, its `.local` name, `localhost` and the bound IPs), stored in `<state>/peer-tls/` with mode 0600, valid 10 years. The fingerprint is the SHA-256 of the SubjectPublicKeyInfo, so reissuing the certificate with the same key keeps it. `cyber peers invite --rotate-key` makes a new key; existing peers then fail with `PeerAuthError { reason: "fingerprint_mismatch" }` until re-added.

Clients use a custom `rustls` verifier that accepts exactly the pinned SPKI hash and ignores the system roots and hostnames, so IP changes and DHCP renames do not break a pairing, and a LAN attacker cannot present any other certificate.

### 2. Authentication: grants with hashed, scoped tokens

```sql
CREATE TABLE peer_grant (
    id            TEXT    PRIMARY KEY,         -- pgr_…
    name          TEXT    NOT NULL UNIQUE,     -- the peer's name on this machine ("laptop")
    token_sha256  TEXT    NOT NULL,            -- hex SHA-256 of the secret
    scopes        TEXT    NOT NULL,            -- JSON array: ["message"] or ["message","attach"]
    created_ms    INTEGER NOT NULL,
    expires_ms    INTEGER,                     -- NULL = no expiry
    last_used_ms  INTEGER,
    revoked_ms    INTEGER
) STRICT;
```

- `cyber peers invite laptop [--scope message|attach]... [--expires 90d]` creates a grant and prints a pairing code once: `cyberpeer://desk.local:4748/?machine=desk&fp=sha256:9f2c…&t=cyp_pgr01H…_<43 base64url chars>`. The secret is 32 random bytes. Only its hash is stored; SHA-256 is enough for 256-bit random secrets (no password stretching is needed), and comparison is constant-time.
- `cyber peers add <name> <code>` (on the other machine) stores the token in the OS keyring (service `cyber-code`, account `peer:<name>`), falling back to `<data>/peers-secrets.json` (0600) with the warning the account spec uses, pins the fingerprint, writes the `remotes` entry, and runs `test`. `{env:NAME}` may replace the keyring for headless machines (`token: "{env:DESK_PEER_TOKEN}"`).
- Requests carry `Authorization: Bearer cyp_…`. The peer guard accepts only `cyp_` tokens; the local password, Unix-socket credentials and Cyber Account tokens are not accepted on the peer listener, and `cyp_` tokens are not accepted on the main API. Any request with an `Origin` header is refused (`403`), so a browser page on the LAN cannot use the listener. Unknown, revoked or expired tokens get `401` with no detail; a valid token without the route's scope gets `403 PeerScopeError`.
- Per-grant rate limit: 120 requests per minute (reads and writes together), `429 RateLimitedError`; the messaging per-sender limits apply on top.
- `cyber peers revoke laptop` sets `revoked_ms`; the next request fails. Pairing in both directions is two invites, one on each machine.

Why tokens over the local password: a grant is a capability for one route group, revocable on its own, and its loss exposes inbox placement (held by default), never shell access.

### 3. Peer identity and addresses

The receiving server builds the sender from the grant, not from the request body: `Sender::Peer { grant: "laptop", session_id: "ses_A", name: Option<String> }`. Its addresses for inbound rules are `ses_A@laptop`, `<name>@laptop` when the sender Session has a unique title, and `*@laptop` via the pattern syntax. The body may carry `from_session` and `from_name`; they are labels inside that grant's namespace, so a compromised laptop can impersonate only its own Sessions, which it controls anyway.

Address tier (design section 3 of the messaging change, step 2): an address `X@P` resolves `P` against `peers.remotes[].name`. A match sends `X` to that peer; no match fails with `PeerNotConfiguredError { peer }` (the `AccountRequiredError` stays for the P3 Relay, reached only when an account is logged in and no peer matches). The remote server resolves `X` with its normal tiers, minus caller-scoped tiers (a peer sender has no subagents or team there).

Replies: the receiving model sees the sender address `review@laptop`. Replying works when the receiving machine has a `remotes` entry named `laptop`, which `cyber peers add` encourages by printing the reverse command (`on laptop: cyber peers invite desk`). Grant names and remote names are compared by the same string; `cyber peers list` flags a grant without a matching remote ("can receive from laptop, cannot reply").

### 4. Peer routes

| Route | Scope | Purpose |
|---|---|---|
| `GET /api/v1/peer/hello` | any | `{ machine, version, fingerprint, scopes, grant }`; used by `test` and the listing health check |
| `GET /api/v1/peer/sessions?limit=50` | message | root Sessions this grant may message (inbound decision is not `refuse`), as listing entries without `location.directory` absolute paths beyond the workspace name unless `peers.share_paths: true` |
| `POST /api/v1/peer/messages` | message | body `{ id: msgx_…, to, text, notify_when_idle?, depth, hops, from_session, from_name? }` → `{ outcome, reason?, target_session_id }` |
| `GET /api/v1/peer/messages/:id` | message | current outcome of a message this grant sent: `delivered|held|released|dropped|refused` |
| `GET /api/v1/peer/sessions/:id/status` | message | `{ state, last_idle_ms }` for watches |
| `GET/POST /api/v1/peer/attach/:id/*` | attach | the Session's event stream, prompt, interrupt and permission-reply routes for one Session (section 7) |

All are in the OpenAPI document under group `peer` and in the TypeScript SDK. `POST /messages` is idempotent on `(grant, id)`: a retry after a timeout returns the first outcome.

### 5. Sends and outcomes

On the sending server, `Messaging::send` with a peer target:
1. Size, self, disable switch, hops and `message.send` permission (default `ask` for `@` targets) run locally, before anything leaves the machine.
2. The peer client posts the message (connect timeout 5 s, total 10 s). Network failure, TLS failure or `5xx` → `PeerUnreachableError { peer, cause }`, nothing written except the sender's `out` record with outcome `failed`; the model decides whether to retry. A fingerprint mismatch is `PeerAuthError` and also marks the remote `state: auth_failed` until fixed.
3. The target runs the full local pipeline with the peer sender: inbound rules (default `hold` for peer senders), limits keyed `peer:<grant>:<ses>`, hops and depth carried from the body, durable `in` record with `origin_kind: "peer"` and `peer_address: "ses_A@laptop"`.
4. The synchronous outcome is recorded in the sender's `out` row (`origin_kind: "peer"`, `peer_address: "migration@desk"`, `remote_session_id`).

`xsession_message` gains `remote_peer TEXT`, `remote_session_id TEXT`, `next_poll_ms INTEGER`. An outcome tracker in the sending server polls `GET /peer/messages/:id` for rows with outcome `held`, backing off 5 s, 15 s, 1 min, then every 5 min, for up to 7 days (then outcome `expired`). A change emits the live `messaging.outcome` event and the durable `messaging.message.released.1` or `messaging.message.dropped.1` in the sender Session (spec "Outcome reporting"). The tracker restarts from `next_poll_ms` after a server restart. Polling is chosen over callbacks because the target usually cannot reach the sender (laptops behind NAT, one-direction grants), and it needs no listener on the sender.

### 6. Listing and watches

`list_sessions` adds, per enabled remote, the entries of `GET /peer/sessions`, fetched in parallel with a 5-second timeout and cached for 15 seconds. Entries have `kind: "peer"`, `address: "<name or ses_>@<peer>"` and the peer's `state`. A peer that does not answer is one entry `{ address: "*@<peer>", kind: "peer", state: "offline", canReceive: false }` (spec scenario "Peer offline"). At most 50 entries per peer.

`watch_session` on a peer target stores a `messaging_watch` row with `target_peer`; a poller calls `/peer/sessions/:id/status` every 15 s while the watch is active and fires when the state goes from `running` to `idle`, the same as local watches. Expiry stays 12 hours. Unreachable peers do not expire a watch early; it fires `expired` at 12 hours.

### 7. Attach

`cyber attach migration@desk` resolves the peer, requires a grant with scope `attach`, and opens the TUI with a transport that maps the TUI's Session routes onto `/api/v1/peer/attach/:id/*`. On `desk`, every attach raises the `remote.attach` permission (resource `peer:laptop`, default `ask`) in that Session; the attach waits up to 2 minutes and fails with `PermissionDeniedError` if refused or unanswered. An `allow` rule (`"remote.attach": { "peer:laptop": "allow" }`) skips the prompt. The attached client acts with the target Session's own permissions and mode; it cannot open other Sessions, change config or reach the shell outside the Session's tools.

### 8. Discovery

With `peers.mdns: true` and a non-loopback listener, the server publishes `_cyber._tcp` (`mdns-sd`) named `cyber-<machine>` with TXT `role=peer`, `v=1`, `machine=<machine>`, `fp=sha256:…`. `cyber peers discover [--timeout 3s]` browses and prints machine, address, fingerprint and whether it is already a remote. It cannot add a peer by itself: `cyber peers add` needs the pairing code, whose fingerprint must match the discovered one when both are present. Default is off because publishing a listener on shared networks advertises the machine. The existing `--mdns` flag of `cyber serve` publishes the same service for the peer listener (the main API is loopback and is never published).

### 9. Surfaces

- TUI: messages from peers render with the machine name (`from review@laptop`); held cards say "from another machine"; the `/messages` view has a peer column; the sidebar's MCP section is unchanged, and a "Peers" line appears only when remotes exist (`desk ●  laptop ○`).
- CLI: `cyber peers list` prints remotes (name, URL, fingerprint prefix, state, latency) and grants (name, scopes, created, last used, expiry). `cyber peers test <name>` calls `hello` and prints machine, version, scopes and latency, exiting 1 on failure with the error tag.
- `cyber doctor` reports the listener address, whether TLS is on, the fingerprint, grant count, and remotes whose token is in the file fallback.

### 10. Security defaults (summary)

Listener off. When on: TLS required off-loopback, only `/api/v1/peer/*`, browser origins refused, local password not accepted. Grants scoped to `message` unless `--scope attach`, hashed at rest, revocable, per-grant rate limit. Peer senders: inbound `hold`, `message.send ask`, `remote.attach ask`, identity assigned by grant. mDNS off. `peers` accepted only from user config. Tokens never logged (they match the `cyp_` redaction pattern added to observability's known key formats).

## Risks / Trade-offs

- **Bearer tokens can be copied.** Anyone who reads the sender's keyring entry can send held messages as that peer. Scoping, hold-by-default and revocation bound the damage; mTLS would not help against a compromised sender either.
- **Polling cost.** A held message polls for up to 7 days at 5-minute intervals (~2,000 requests worst case). Acceptable at this scale; a later push channel can replace it without API changes.
- **No delivery while offline.** A send to an offline peer fails rather than queueing. This keeps semantics simple (the model sees the failure) and avoids a second outbox state machine.
- **Clock skew.** Expiry and backoff use each machine's own clock; only `expires_ms` of grants is compared to the target's clock.

## Migration Plan

Additive: new table, new nullable columns, new config key (already accepted by `validate.rs`), new routes. The old `peers` list shape was never implemented, so no config migration is needed; a `peers` array in config fails validation with a message pointing to `cyber peers add`.

## Open Questions

None blocking. `peers.share_paths` defaults to `false` (only the workspace name is shared); revisit if users want full paths in listings.
