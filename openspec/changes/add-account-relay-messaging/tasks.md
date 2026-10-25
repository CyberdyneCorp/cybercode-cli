Proposed 2026-10-24. Implement after the M3.1 account foundation shipped by `add-project-collaboration-hub`. Archive after `add-project-collaboration-hub`, which also modifies `cross-session-messaging` "Cross-machine transport".

Each numbered section is one increment with its own tests. Tests run without external network:
- a `cyber relay serve` started in-process or as a binary on `127.0.0.1`;
- a fixture OIDC issuer;
- two or three `cyber` servers with separate state, data and config directories;
- the scripted providers in `crates/cyber-app/tests/support/scripted_model.rs`.

## 1. Config, errors and policy

- [ ] 1.1 `messaging.relay { enabled, url, machine, share_with_org }`, from user config and managed policy only (a project or local layer fails naming its path). Add `policy.messaging.relay` and `policy.messaging.org_members`. Unit tests for each rule.
- [ ] 1.2 Error tags `MachineUnreachableError { machine }`, `MachineNameTakenError { machine }` and `MachineNotApprovedError { machine }` in the tagged error model, OpenAPI and SDK.

## 2. Machine identity

- [ ] 2.1 X25519 key and `mch_` ID in `<state>/machine/` (mode 0600, refused when readable by others); name normalization; fingerprint. Tests: stable across restarts; file modes on Unix.

## 3. Relay (messaging subset)

- [ ] 3.1 New crate `cyber-relay`:
  - WSS listener;
  - token verification against the issuer (scopes `cyber:relay` and `cyber:messaging`);
  - an account roster with presence and name-clash refusal;
  - org rosters gated by policy and member opt-in;
  - frame routing between machines, never decrypting;
  - limits of 120 frames per minute, 64 KiB per frame and 32 connections per account.
- [ ] 3.2 `cyber relay serve --listen --issuer [--no-entitlement-check]`, plus the `cyber-code` entitlement check otherwise.
- [ ] 3.3 Tests:
  - the Relay never holds plaintext: a captured frame contains no Session title or message text;
  - an injected roster entry reaches clients only as pending.

## 4. Relay client connection

- [ ] 4.1 Outbound WSS from the server when enabled and signed in: announce, ping every 20 s, jittered reconnect, close on logout, shared token refresh.
- [ ] 4.2 Add Relay state, machine ID, fingerprint and approved and pending counts to `cyber doctor`.

## 5. Trust approval

- [ ] 5.1 Migration `machine_trust`. Pending entries from roster pushes; the six-word code from the sorted public keys; approval needed from both sides; reset to pending on a key change; `reject` tombstones; `forget` closes channels.
- [ ] 5.2 Tests for each scenario of "Machine trust approval".

## 6. Encrypted channel

- [ ] 6.1 The handshake and frames of `remote-control` "End-to-end encryption", between machine keys: ephemeral X25519, HKDF-SHA256, ChaCha20-Poly1305 with a monotonic nonce, and closing after 3 replays.
- [ ] 6.2 Typed frames reusing the peer route shapes: hello, sessions, send, status, outcome and watch-status.

## 7. Addressing, delivery and outcomes

- [ ] 7.1 Address tiers `<session>@<machine>` and `<session>@<member>/<machine>` after peer remotes; ambiguity with peer names; sender identity assigned from the channel.
- [ ] 7.2 Target pipeline with a Relay sender kind: hold by default for the own account; always hold across accounts; recheck on release.
- [ ] 7.3 Sender outcomes through the shared peer outcome tracker; `MachineUnreachableError` after 5 s; no queue.

## 8. Listing and watches

- [ ] 8.1 Relay fan-out in `list_sessions` and `GET /api/v1/messaging/sessions` (5 s timeout, 15 s cache, at most 50 per machine, `kind: relay`, offline `*@machine` entries).
- [ ] 8.2 `watch_session` for Relay addresses: a 15 s status poll that expires after 12 hours.

## 9. Surfaces

- [ ] 9.1 Machines routes in `server-api` and the OpenAPI document, with TypeScript SDK methods.
- [ ] 9.2 `cyber machines list|whoami|rename|approve|reject|forget`, and regenerate `docs/cli.md`.
- [ ] 9.3 TUI: the `⚇ <n> pending` footer badge, a pending-machine card (`a` approves, `x` rejects) and machine badges on held cards.

## 10. Docs

- [ ] 10.1 New `docs/relay-messaging.md`; a "peers or Relay" section in `docs/peers.md`; the README collaboration section; `docs/features.md`; ROADMAP M3.3 status.

## 11. Acceptance

- [ ] 11.1 Real binaries:
  - two machines of one account behind a local Relay;
  - enable, approve both sides, list, send, hold, release and observe the outcome.
- [ ] 11.2 Three machines, two accounts in one org:
  - blocked without the policy;
  - blocked without the member opt-in;
  - allowed and always held with both.
- [ ] 11.3 Offline target returns `MachineUnreachableError`; a key change resets trust; `forget` closes the channel.
