Specified 2026-10-18. Sections 1 to 10 and 12 implemented 2026-10-19; section 11 is partial (see the notes under 11.1 and 11.2). Implement after `implement-cross-session-messaging` (it owns the pipeline, records, tools and routes extended here); archive after it and after `add-session-rename` (both modify `cli-commands` "Resource command groups").

Each numbered section is one increment with its own tests. Tests run locally with no external network: two-server tests start two real servers in one process or as two `cyber` binaries with separate state, data and config directories, peer listeners on `127.0.0.1` with TLS on, and the scripted providers of `crates/cyber-app/tests/support/scripted_model.rs`. New test files: `crates/cyber-server/tests/peers.rs`, `crates/cyber-app/tests/peers.rs`, `crates/cyber-cli/tests/peers_cli.rs`.

## 1. Config and errors

- [x] 1.1 `peers` config in `crates/cyber-core/src/config/` (design section 1): `listen { address, tls }`, `mdns`, `machine`, `share_paths`, `remotes[] { name, url, fingerprint, enabled, token? }`; accepted only from user config (project and local config fail with a message); `tls: false` off loopback, non-loopback `http` remote URLs, duplicate names and the old array shape rejected with messages naming `cyber peers add`. Unit tests for each rule.
- [x] 1.2 Error tags `PeerUnreachableError { peer, cause }`, `PeerAuthError { peer, reason }`, `PeerNotConfiguredError { peer }`, `PeerScopeError { scope }` in the tagged error model, OpenAPI and SDK; `cyp_` added to the redaction patterns with a redaction unit test.

## 2. TLS identity and the peer listener

- [x] 2.1 `http/peer/tls.rs`: generate and load the ECDSA P-256 key and self-signed certificate (`rcgen`) in `<state>/peer-tls/` with mode 0600, SANs per design section 1, SPKI SHA-256 fingerprint, `--rotate-key`. Tests: fingerprint stable across certificate reissue with the same key; file modes on Unix.
- [x] 2.2 Second listener (`tokio-rustls`) started and stopped with the server; serves only `/api/v1/peer/*` (everything else `404`); refuses any `Origin` header with `403`; `peer_url` and fingerprint in `server.json`; plain mode only on loopback.
  - Production path: `crates/cyber-app/tests/peers.rs` `the_peer_listener_serves_only_peer_routes` (scenarios "Only peer routes are served", "Plain listener off loopback", "Browser request").

## 3. Grants and the peer guard

- [x] 3.1 Migration `peer_grant` (design section 2); `runtime/peers/grants.rs` create (token generation, SHA-256 hash), verify (constant-time), revoke, expire, `last_used_ms` update.
- [x] 3.2 Peer guard: only `Bearer cyp_…` on the peer listener; `cyp_` refused on the main API; `401` for unknown, revoked or expired; `403 PeerScopeError` per route scope; 120 requests per minute per grant (`429`).
- [x] 3.3 `GET /api/v1/peer/hello`.
  - Production path: `crates/cyber-server/tests/peers.rs` `grants_authenticate_scope_and_revoke` (scenarios "Revoked grant", "Message grant cannot attach"; the database holds the hash, not the token).

## 4. Peer client and `cyber peers`

- [x] 4.1 `runtime/messaging/peer.rs` client: `reqwest` with a custom `rustls` verifier that accepts exactly the pinned SPKI hash; connect 5 s, total 10 s; maps failures to `PeerUnreachableError` and `PeerAuthError { reason: fingerprint_mismatch | unauthorized | scope }`. Test against a server presenting another key.
- [x] 4.2 Token storage: keyring (`cyber-code`, `peer:<name>`), 0600 file fallback with warning, `{env:NAME}`.
- [x] 4.3 `crates/cyber-cli/src/commands/peers.rs`: `invite` (pairing code format of design section 2; `--rotate-key` calls `PeerIdentity::rotate` of 2.1 and swaps the running listener's certificate), `add`, `rm`, `list [--json]`, `test`; the reverse-pairing hint; the "can receive, cannot reply" flag.
  - Production path: `crates/cyber-cli/tests/peers_cli.rs` `two_binaries_pair_and_test` (scenario "Pair two machines": two real `cyber` servers, `invite` on one, `add` and `test` on the other).

## 5. Sends between servers

- [x] 5.1 Address tier: `X@P` resolved against `peers.remotes`; `PeerNotConfiguredError`; `AccountRequiredError` kept for the Relay case only.
- [x] 5.2 `Sender::Peer { grant, session_id, name }` with its addresses for inbound rules; `origin_kind: "peer"`; inbound default `hold`; limits counting every Session of the grant together (`peer:<grant>:*`); hops and depth from the body.
- [x] 5.3 `POST /api/v1/peer/messages` (idempotent on grant and ID) and `GET /api/v1/peer/messages/:id`; sender-side `out` records with `remote_peer`, `remote_session_id`, outcome `failed` on network errors.
  - Production path: `crates/cyber-app/tests/peers.rs` `a_message_to_a_peer_is_held_there_and_named_by_grant` (scenarios "Two laptops on one network", "Sender cannot claim another machine", "Unknown peer name", "Retry after a timeout").

## 6. Outcome tracking

- [x] 6.1 Migration: `remote_peer`, `remote_session_id`, `next_poll_ms` on `xsession_message`; tracker task with the backoff of design section 5, 7-day expiry to `expired`, resume after restart.
- [x] 6.2 Live `messaging.outcome` and durable `messaging.message.released.1` / `dropped.1` in the sender Session.
  - Production path: `crates/cyber-app/tests/peers.rs` `a_peer_release_reaches_the_sender_across_a_restart` (scenario "Release seen by the sender", with a test clock for the backoff).

## 7. Listing and watches

- [x] 7.1 `GET /api/v1/peer/sessions` (grant-visible root Sessions, workspace-name locations unless `share_paths`, 50 cap); `list_sessions` fan-out with 5 s timeout, 15 s cache, offline entry.
- [x] 7.2 `GET /api/v1/peer/sessions/:id/status`; remote watches with `target_peer`, 15 s polling, 12-hour expiry.
  - Production path: `crates/cyber-app/tests/peers.rs` `peer_sessions_are_listed_and_watched` (scenarios "Peer offline", "Refused grant hides Sessions", "Watch a remote Session").

## 8. Discovery

- [x] 8.1 `mdns-sd` publish of the peer listener (TXT of design section 8) when `peers.mdns` or `cyber serve --mdns`, unpublish on shutdown; loopback skip with a warning.
- [x] 8.2 `cyber peers discover [--timeout]`; `add` refuses a code whose fingerprint differs from a discovered one for the same address.
  - Production path: `crates/cyber-cli/tests/peers_cli.rs` `discover_lists_a_published_peer` (scenario "Discover a desktop"; skipped with a logged reason where multicast is unavailable, as on CI runners without it).

## 9. Attach

- [x] 9.1 `/api/v1/peer/attach/:id/*` for one Session's event stream, prompt, interrupt and permission replies; `remote.attach` request with resource `peer:<grant>`, 2-minute wait.
- [x] 9.2 `cyber attach <session>@<peer>` transport in `crates/cyber-client` and the TUI.
  - Production path: `crates/cyber-app/tests/peers.rs` `attach_through_a_peer_needs_scope_and_approval` (scenarios "Attach approved", "Attach without approval", "Attach to a peer Session").

## 10. Surfaces and docs

- [x] 10.1 TUI: peer sender labels, held cards naming the machine, peer column in `/messages`, sidebar "Peers" line when remotes exist; `insta` snapshots.
- [x] 10.2 `cyber doctor` peer section; OpenAPI group `peer`; regenerated TypeScript SDK.
- [x] 10.3 README "Messaging across machines" section (pairing, SSH-tunnel setup, security defaults), `docs/features.md`, `docs/implementation/p2-status.md`.

## 11. Acceptance

- [ ] 11.1 Two real `cyber` binaries with separate homes on one machine, peer listeners on `127.0.0.1` with TLS: invite and add both ways, a Session on A sends to a named Session on B, the TUI on B shows the held card, the user releases it, B's model replies to A, A sees the release and the reply; then `revoke` on B and the next send from A fails with `PeerAuthError`. Real-binary pty captures of both TUIs.
  - Partial (2026-10-19, by hand, evidence in `docs/implementation/p2-status.md`): two real `cyber serve` instances with separate homes, TLS listeners on `127.0.0.1`, scripted provider. Done: invite and add (one way), send to a named Session held on B, release in B's TUI, A's polled outcome `released` (also across an A restart), revoke then `PeerAuthError` on the next send, pty captures of both TUIs, plus attach, discovery, spoofing, fingerprint and listener checks. Not done: the reverse pairing and B's model replying to A; the keyring was mocked with `--token-env`.
- [ ] 11.2 The same flow with B's peer listener plain on loopback behind an SSH-style local port forward (a TCP proxy in the test), confirming the tunnel setup.
  - Not done: the plain loopback listener is tested (`a_plain_peer_listener_opens_only_on_loopback`), but no test or manual run goes through a forwarded port.

## 12. Review fixes

- [x] 12.1 A peer's `to` resolves only among the Sessions the grant is shown: hidden Sessions are `AddressNotFoundError`, ambiguous candidates are filtered and located by workspace name unless `peers.share_paths`. Test: `crates/cyber-server/tests/peers.rs` `peer_sends_never_reveal_sessions_hidden_from_the_grant`.
- [x] 12.2 An attached peer's `always` permission reply is refused (`InvalidRequestError`); no saved approval outlives the attach. Test: `crates/cyber-app/tests/peers.rs` `an_attached_peer_cannot_approve_always`.
- [x] 12.3 The outcome tracker and the remote watch poller ask peers in parallel, one client and at most 5 polls per peer per pass, skip a peer after its first failure, and reschedule a change that cannot be recorded. Tests: `crates/cyber-app/tests/peers.rs` `one_silent_peer_does_not_delay_a_release_on_another`, `tracker::tests::a_pass_takes_a_few_due_messages_of_each_peer`.
- [x] 12.4 Only a `deliver` pattern naming a peer (with `@`) delivers a cross-machine sender; local patterns and a Session override `deliver` still hold. Tests: `crates/cyber-app/tests/peers.rs` `local_deliver_patterns_still_hold_peer_messages`, `inbound::tests::only_a_pattern_naming_a_peer_delivers_a_cross_machine_sender`.
- [x] 12.5 The peer listener serves HTTP/1.1 only, at most 64 connections, with a 10 s header (and idle) timeout, and refuses an address with 30 failed tokens a minute before any lookup. Tests: `crates/cyber-server/tests/peers.rs` `peer_connections_are_capped_and_time_out`, `invalid_peer_tokens_are_limited_per_address`.
- [x] 12.6 Peer message limits count every Session of a grant together. Tests: `crates/cyber-server/tests/peers.rs` `peer_limits_count_every_session_of_the_grant`, `limits::tests::a_prefix_counts_every_session_of_one_grant`.
- [x] 12.7 `503 ServiceUnavailableError` from a peer is reported as `ServiceUnavailableError`, not `PeerUnreachableError`, and records nothing. Test: `crates/cyber-app/tests/peers.rs` `a_peer_with_messaging_disabled_says_so`.
- [x] 12.8 A wildcard listener's pairing code names this host's LAN name, also advertised in the mDNS TXT `host`; `invite` hints at `--host` when `peers.machine` differs. Tests: `crates/cyber-app/tests/peers.rs` `a_wildcard_pairing_code_names_the_hosts_lan_name` and `mdns_publishes_the_peer_listener_only_off_loopback`.
