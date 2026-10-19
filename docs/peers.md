# Direct peers: messaging across your machines

Direct peers let Sessions on your own machines message each other without an account or a relay. A Session on `laptop` sends to `migration@desk`, `list_sessions` shows desk's Sessions as `<name>@desk`, `watch_session` works on them, and `cyber attach migration@desk` opens one of them in the local TUI. The contract is the `cross-session-messaging` spec ("Direct peer transport" and the peer requirements) and the change [`add-messaging-direct-peers`](../openspec/changes/add-messaging-direct-peers/proposal.md). Same-server messaging is described in the [local core guide](local-core-guide.md#cross-session-messaging).

Status (2026-10-19): implemented, and accepted by hand with two real `cyber serve` instances with separate homes on one macOS host, peer listeners on `127.0.0.1` with TLS on, against a scripted local provider. See [Evidence and limits](#evidence-and-limits) for what that run did not cover.

## How it fits together

- **Receiver.** A server that should receive opens a second listener (`peers.listen`). It is separate from the main API, which stays on loopback, and it serves only `/api/v1/peer/*`. Peers authenticate there with a grant token (`cyp_…`) that this machine issued.
- **Sender.** A server sends through the peer listener of a configured remote (`peers.remotes`). It accepts only the certificate whose public key matches the fingerprint pinned at pairing, and it sends the token kept in the OS keyring.
- **Grants go one way.** `desk` inviting `laptop` lets laptop send to desk. For desk to reply, laptop invites desk too. `cyber peers list` on the receiver marks a grant whose peer it cannot reply to ("can receive, cannot reply") and prints the reverse-pairing command.

## Configure the listener

`peers` is read only from user config (`~/.config/cyber/cyber.jsonc`). Project, local, environment, profile and `--config` layers that set it fail with `ConfigInvalidError`.

```jsonc
// desk: user config
{
  "peers": {
    "listen": { "address": "0.0.0.0:4748" },   // no listener opens without this
    "machine": "desk",                         // the name peers see in `hello`
    "mdns": false,                             // publish over mDNS (off by default)
    "share_paths": false                       // listings show workspace names, not paths
  }
}
```

`remotes` is written by `cyber peers add`. Edit it by hand only to disable a remote (`"enabled": false`).

On first start the server creates an ECDSA P-256 key and a self-signed certificate in `<state>/peer-tls/` (`cert.pem` and `key.pem`, mode 0600). `cyber doctor` reports the listener's address, whether TLS is on, the key fingerprint, the number of active grants, and any remotes whose token is in the file fallback:

```
peers:listener: pass https://127.0.0.1:48701 · TLS on · fingerprint sha256:d15c0f7f… · 0 active grants
```

## Pair two machines

```bash
desk$   cyber peers invite laptop
cyberpeer://desk-host.local:4748/?machine=desk&fp=sha256:…&t=cyp_…
on laptop: cyber peers add desk '<code>'

laptop$ cyber peers add desk '<code>'
added desk (https://desk-host.local:4748, sha256:d15c0f7f7cd3…): machine desk, version 0.1.0, scopes message, 28 ms
laptop$ cyber peers test desk        # machine, version, scopes, latency, and the name desk gives laptop
laptop$ cyber peers list             # remotes: online/offline and latency; grants this machine issued
```

- **The code** is printed once. It holds the listener URL, the key fingerprint and a 256-bit token. The receiver stores only the token's SHA-256 hash. For a listener on all interfaces, the URL names this host's LAN name (`<hostname>.local`). Use `--host <IP or name>` when the other machine cannot resolve it, for example a Tailscale name.
- **Invite options.** `--scope message` is the default. Add `--scope attach` (repeatable) to allow `cyber attach`. `--expires 90d|12h|30m` ends the grant automatically. `--rotate-key` replaces the listener key first, and every existing pairing must then be redone.
- **`add`** calls the peer's `hello` before it stores anything. A wrong fingerprint or a refused token fails with `PeerAuthError` (`fingerprint_mismatch` or `unauthorized`), writes nothing, and the hint says to ask for a new code. On success it pins the fingerprint, writes the remote to user config, and keeps the token in the OS keyring (service `cyber-code`, entry `peer:<name>`). Where no keyring exists, the token goes to an owner-only file and the command warns. With `--token-env NAME`, config gets `{env:NAME}` and the token is never written: `NAME` must then hold the token wherever the server runs. A running server keeps the environment it started with, so restart it after re-pairing this way.
- **`cyber peers rm <name>`** forgets a remote and its stored token. **`cyber peers revoke <name>`**, run on the receiver, ends a grant at once. The peer's next request is refused with `PeerAuthError (unauthorized)`, and its open attach streams end.

## Sending, holding and outcomes

- **Addresses.** `X@P` resolves `X` among the Sessions of the enabled remote named `P`. A peer name that no remote has fails with `PeerNotConfiguredError`, and nothing is sent. The `message.send` permission asks for every `@` target.
- **Sender identity.** The receiver names the sender after the grant that authenticated it, as `<session>@<grant name>` (`review@laptop`). Request fields cannot change it: an extra `from` field is rejected as an unknown field, a `from_name` containing `@` is rejected, and a plain `from_name` keeps the grant's machine part.
- **Held by default.** A message from another machine is held in the target's inbox until the user releases or drops it: in the TUI, the held card says "from another machine", and `Tab` then `r` releases it. It can also be released with `POST /api/v1/sessions/{id}/inbox/{message_id}/release`. Only a `messaging.inbound` `deliver` pattern that names a peer (`*@laptop`) delivers without holding. `*` and other patterns without `@` never deliver a cross-machine sender, and neither does a Session override to `deliver`. Delivered text is an untrusted `<cross_session_message>` block, as on one machine.
- **Outcomes.** The send returns `held`. The sender polls the outcome every 5 s, backing off to every 5 min, for up to 7 days, after which the outcome is `expired`. Polling resumes after a sender restart. A release shows as outcome `released`, together with `messaging.message.released.1` in the sender Session's history and the live `messaging.outcome` event.
- **No store-and-forward.** A peer that does not answer within 5 s fails the send with `PeerUnreachableError`. The record is `failed / unreachable`, and nothing is queued or retried. A peer whose messaging is turned off answers `ServiceUnavailableError`.
- **Limits.** Size, rate and chain limits apply as on one machine, and all Sessions of one grant count as one sender.

## Listing, watching and attaching

- `list_sessions` (and `cyber messages list --reachable`) asks each enabled remote, with a 5 s timeout and a 15 s cache. A reachable peer's root Sessions appear as `<name>@<peer>` with `kind: peer`. Their location is the workspace name only, unless the receiver sets `peers.share_paths`. A peer sees and can address only the Sessions that would accept its messages, at most 50. An unreachable peer appears as a single `*@<peer>` entry with state `offline`.
- `watch_session` on `X@P` polls every 15 s and expires after 12 hours.
- `cyber attach <session>@<peer>` needs a grant with the `attach` scope. It raises a `remote.attach` request (resource `peer:<grant>`) in that Session on the receiver, which asks by default and waits up to 2 minutes. A refusal fails with `PermissionDeniedError`. Once approved, the local TUI shows that Session's events and can prompt it, interrupt it, and answer its permission requests, but only `once`, never `always`. It needs a terminal (exit 2 otherwise).

## Over an SSH tunnel

Without a LAN or VPN between the machines, keep the listener on loopback and forward the port. Plain HTTP is accepted only on a loopback address, and the tunnel provides the encryption:

```jsonc
// desk: user config
{ "peers": { "listen": { "address": "127.0.0.1:4748", "tls": false }, "machine": "desk" } }
```

```bash
laptop$ ssh -N -L 4748:127.0.0.1:4748 desk   # keep it running
desk$   cyber peers invite laptop             # cyberpeer://127.0.0.1:4748/?…&tls=0
laptop$ cyber peers add desk '<code>'
```

`tls: false` on a non-loopback address, and an `http://` remote URL off loopback, are rejected in config. The plain listener on loopback is covered by `crates/cyber-app/tests/peers.rs` `a_plain_peer_listener_opens_only_on_loopback`. The tunnel flow itself has not been run (task 11.2).

## Discovery

With `peers.mdns: true` (or `cyber serve --mdns`), the server publishes its peer listener over mDNS. It does not publish a loopback listener, and logs a warning instead. `cyber peers discover [--timeout 5s] [--json]` lists the machine, address and fingerprint of each published listener, and whether it is already a remote. Discovery never pairs and never grants trust, so a code from `invite` is still needed. `add` refuses a code whose fingerprint differs from the one discovered at the same address.

## Security model

- **Off by default and fail closed.** No listener opens unless `peers.listen` is set. With an invalid setting, `tls: false` off loopback, a bad key or a busy port, the listener stays closed and the main API starts without it.
- **Transport.** Off loopback, the listener serves only TLS 1.3, HTTP/1.1, at most 64 connections, with a 10 s header and idle timeout. Senders verify only the pinned SPKI SHA-256 fingerprint, not a CA chain.
- **Authentication.** On the peer listener, only `Bearer cyp_…` grant tokens are accepted, and the local server password is not. `cyp_` tokens are refused on the main API (`401`). Tokens are compared in constant time against their SHA-256 hashes. Unknown, revoked and expired grants get `401`, and a route outside the grant's scope gets `403 PeerScopeError`.
- **Abuse limits.** Each grant gets 120 requests a minute (`429` after that). An address that presents 30 invalid tokens in a minute gets `429` before any lookup. Requests with an `Origin` header (browser pages) get `403`, and every non-peer route is `404`.
- **Trust stays on the receiver.** Senders are named by the grant and held by default. Permissions, Mode ceilings, sandboxing and the inbound rules of the receiving Session apply unchanged. `remote.attach` needs an explicit approval, and an attached peer cannot save an approval.
- **Secrets.** Tokens go to the OS keyring, an owner-only file, or an environment variable, never to config. They match the `cyp_` redaction pattern and are kept out of logs, transcripts and commit messages.

## API and SDK

The peer routes (`/api/v1/peer/hello`, `/peer/messages`, `/peer/messages/:id`, `/peer/sessions`, `/peer/sessions/:id/status`, `/peer/attach/:id/*`) are in the OpenAPI document under group `peer` and in the TypeScript SDK as `client.peer`, with the grant token passed as `auth: { type: "account", token }`. The local management routes (`GET /api/v1/peers`, `/peers/remotes`, `POST /peers/grants`, `DELETE /peers/grants/:name`) are `client.peers`. The errors are `PeerUnreachableError` (`502`), `PeerAuthError` (`502`), `PeerNotConfiguredError` (`404`) and `PeerScopeError` (`403`).

## Evidence and limits

The manual acceptance on 2026-10-19 used two real `cyber serve` instances, `desk` and `laptop`, with separate HOME/XDG directories on one host, peer listeners on `127.0.0.1:48701/48702` with TLS, and a scripted local OpenAI-compatible provider. It covered:

- invite, add (fingerprint pinned, raw token absent from config), list and test
- a held send, its release in desk's TUI and the sender's polled outcome, including while the sender was down and after it restarted
- spoofing attempts against the sender identity
- listing desk's Sessions, and desk offline after it stopped
- `PeerUnreachableError` with nothing queued
- a wrong fingerprint (on `add`, on an edited pin, and with curl)
- revocation taking effect on the next request
- attach refused without the scope, rejected, and approved
- mDNS discovery on `0.0.0.0` from a paired home and from an unpaired third home, whose config stayed unchanged
- the listener's `Origin`, route, token and rate-limit checks
- pty captures of both TUIs

Three bugs found in that run were fixed with regression tests: an attached TUI that never saw a Turn end, a doubled error tag, and a misleading hint after a refused `add`.

Not covered or not done:

- **OS keyring.** The keyring was not exercised end to end. The run used the `--token-env` store, and the keyring path is covered only by `peer_tokens` unit tests against an in-memory vault.
- **Other hosts and platforms.** Only one host was used: no run across two physical machines, and none on Linux or Windows.
- **SSH tunnel.** The tunnel flow (task 11.2) has not been run.
- **Two-way pairing (task 11.1).** The run paired one way only. A reply from desk's model reaching laptop through a reverse grant was not shown.
- **Received messages in an attached TUI.** These show as raw `<cross_session_message …>` text, not the `✉ from …` block, because the attach routes do not include the messaging-history route.
- **Waiting message on a missing scope.** `cyber attach` prints "waiting for … to approve" even when the grant lacks the `attach` scope and the request then fails at once.
- **Paths in an attached TUI.** The attached TUI's sidebar shows the receiver's full Location path. Listings show workspace names only.
- **`--token-env` re-pairing.** The server must be restarted to pick up the new token.
