## Why

Today a Session can reach another Session on the same server, on a machine paired as a direct peer, or through an experimental project hub. Each path beyond one machine needs manual setup. Direct peers need a reachable address (same LAN, a VPN or an SSH tunnel) and a pairing code copied for each pair of machines. The hub needs a self-hosted deployment and an OIDC identity provider. A user with a laptop at home and a workstation at the office, on different networks, cannot simply see and message their own agents. A small team cannot do so either without running the hub.

The specs already describe the intended answer: the Cyber Account (M3.1), an outbound-only Relay with end-to-end encryption (M3.2) and "Cross-machine transport" over that Relay (M3.3). The parts that let machines find each other are not specified concretely. Nothing says how a server announces itself, how another machine of the same account learns its key and name, how the user is protected from a Relay that injects a fake machine, how teammates' machines are named, or how a Relay connection used only for messaging relates to `/remote`. This change specifies that path so it can be implemented ahead of the rest of M3.2 (paired Devices, mobile apps, push notifications).

## What Changes

- **Machine identity.** Each server gets a long-term X25519 machine key and an ID with prefix `mch_`. The machine name defaults to the host name and can be renamed with `cyber machines rename`.
- **A messaging-only Relay connection.** `messaging.relay.enabled` (off by default, user config only) makes a logged-in server dial the Relay over WSS. It authenticates with scopes `cyber:relay` and `cyber:messaging` and announces its `mch_` ID and public key. This connection does not expose any Session for remote control; `/remote` stays a separate opt-in.
- **An account roster on the Relay, with names kept end-to-end.** The Relay keeps a roster of each account's machines: ID, public key, online state and last-seen time. Machine names, Session titles and listings travel only inside the end-to-end encrypted channel, so the Relay never sees them.
- **Explicit machine trust.** A machine that appears in the roster is `pending` until the user approves it on both sides. Approval compares a six-word code derived from both public keys, shown by `cyber machines` and the TUI. This keeps a malicious or compromised Relay from inserting itself. `cyber machines forget` revokes trust at once.
- **Discovery without pairing codes.** `list_sessions`, `cyber messages list --reachable` and the TUI agent directory include approved machines of the same account. Their reachable root Sessions appear as `<session>@<machine>`, with offline machines shown as `*@<machine>`.
- **Team reach, opt-in on every side.** Members of a shared org can reach each other's machines only when all of these hold:
  - an org admin sets `policy.messaging.org_members: true`;
  - each member opts in with `messaging.relay.share_with_org: true`;
  - both sides approve the machine pair.

  A teammate's Sessions are addressed as `<session>@<member>/<machine>`. Messages from other accounts are always held; no rule can deliver them without the receiver releasing them.
- **Same message pipeline.** A message over the Relay keeps the existing limits on size, rate and chain length, the receiver's inbound rules, its untrusted rendering, its durable records and its asynchronous outcomes. Same-account machines are held by default as with direct peers. A `deliver` pattern naming the machine (`*@laptop`) delivers without holding. There is no store-and-forward: an offline machine fails the send with `MachineUnreachableError`.
- **A self-hostable messaging Relay.** `cyber relay serve` gains the roster and message routing described here, so a team can run its own Relay. The hosted Relay requires the `cyber-code` entitlement; a self-hosted Relay may skip that check.
- **CLI.** `cyber machines list|whoami|rename|approve|reject|forget`.

Out of scope:
- **Relay remote control:** paired Devices, the Relay web client, mobile apps and push notifications all stay in M3.2.
- **Offline delivery:** store-and-forward queues, NAT hole punching and direct connections between machines (all traffic goes through the Relay).
- **Other transports:** messages to accounts outside a shared org, attachments in messages, and attach over the Relay (a follow-up that reuses "Peer attach").
- **The TUI agent directory:** it is a separate P1 change. It consumes the listing this change adds, but does not depend on it.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `cross-session-messaging`: "Cross-machine transport" is rewritten (machine identity, roster, approval, team addressing, no store-and-forward). Three requirements are added: "Machine trust approval", "Relay message delivery" and "Relay reachable listing".
- `remote-control`: "Messaging-only Relay connection" and "Relay machine roster" are added.
- `cli-commands`: "Machines command" is added.

## Impact

- **Code:**
  - a new `crates/cyber-relay` crate (roster, frame routing, the `cyber relay serve` messaging subset);
  - `crates/cyber-server/src/runtime/messaging/` (relay client, address tier, outcome tracking shared with peers);
  - `crates/cyber-core/src/config/` (`messaging.relay`);
  - `crates/cyber-store/migrations` (`machine`, `machine_trust`);
  - `crates/cyber-cli/src/commands/machines.rs`;
  - `crates/cyber-tui` (pending-approval prompt, machine badges);
  - `sdk/openapi.json` and `sdk/typescript`.
- **Dependencies:** `x25519-dalek`, `chacha20poly1305` and `hkdf` (the same primitives the "End-to-end encryption" requirement names); `tokio-tungstenite` for WSS.
- **Identity provider:** scopes `cyber:relay` and `cyber:messaging` (ROADMAP dependency A2), and a confidential Relay client with an access-token audience (A4).
- **Docs:** README (Sessions and people working together), `docs/peers.md` (when to use peers instead of the Relay), a new `docs/relay-messaging.md`, `docs/features.md` and ROADMAP M3.3.
- **Sequencing:** implement after the M3.1 account foundation that `add-project-collaboration-hub` ships. Archive after `add-project-collaboration-hub`, which also modifies "Cross-machine transport".
