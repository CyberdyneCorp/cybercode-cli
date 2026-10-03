# remote-control Specification

## Purpose
Remote control lets the user drive a Session running on their own machine from a phone, a browser or another computer, while code execution, files and MCP servers stay local. It brings Claude Code's Remote Control to an open stack. The relay is self-hostable, and traffic is end-to-end encrypted between the Device and the user's server, so the relay never sees code or prompts. It reuses OpenCode's attach model and v2's durable replay for lossless reconnection.

## Requirements

### Requirement: Enabling remote control
(P3) The system SHALL provide `cyber remote enable [--session <id>]`, the TUI command `/remote`, and the config key `remote.auto: true` (expose every Session). Enabling SHALL require a logged-in Cyber Account and SHALL connect the server to the configured Relay. `cyber remote disable` and `/remote off` SHALL stop exposing the Session or Sessions immediately.

#### Scenario: Enable without login
- **WHEN** the user runs `/remote` while logged out
- **THEN** the TUI offers to run `cyber login` and does not connect to the Relay

#### Scenario: Expose one session
- **WHEN** the user runs `/remote` in a Session
- **THEN** only that Session becomes visible to the user's paired Devices

### Requirement: Outbound-only relay connection
(P3) The local server SHALL dial out to the Relay over WSS (`remote.relay_url`, default the Cyber Cloud relay) and SHALL NOT open inbound ports. The connection SHALL authenticate with a Cyber Account access token carrying scope `cyber:relay`, and SHALL send a WebSocket ping every 20 seconds.

#### Scenario: Behind NAT
- **WHEN** the machine is behind NAT with no port forwarding
- **THEN** remote control works because only an outbound WSS connection is used

### Requirement: Self-hosted relay
(P3) The system SHALL provide `cyber relay serve --listen <addr> --issuer <discovery url>` to run a Relay that verifies account tokens against the configured CyberdyneAuth issuer. The Relay SHALL serve the web client at `/`. A self-hosted relay MAY set `--no-entitlement-check` to skip the `cyber-code` entitlement requirement.

#### Scenario: Company relay
- **WHEN** an org sets `policy.remote.relay_url` to its self-hosted relay
- **THEN** all members' servers connect to that relay and the user cannot override it

### Requirement: End-to-end encryption
(P3) Traffic between a Device and the local server SHALL be end-to-end encrypted. The parties SHALL authenticate with long-term X25519 device and server keys, use ephemeral X25519 keys per connection, and derive session keys with HKDF-SHA256 for ChaCha20-Poly1305 frames carrying a monotonic nonce. The Relay SHALL see only ciphertext plus routing metadata (account, server ID, device ID, frame size and time), and SHALL never be able to decrypt payloads.

#### Scenario: Relay operator inspects traffic
- **WHEN** the relay operator captures frames for a remote Session
- **THEN** the frames contain only ciphertext and routing metadata

#### Scenario: Replayed frame
- **WHEN** a frame with an already-seen nonce is delivered again
- **THEN** the receiver discards it and closes the channel after 3 such frames

### Requirement: Device pairing
(P3) `cyber remote pair` SHALL display a QR code and an 8-character code, valid for 5 minutes, that binds a new Device to the account and exchanges its public key with the server. Pairing SHALL require the Device to be logged in to the same Cyber Account. Each paired Device SHALL get an ID with prefix `dev_`.

#### Scenario: Pair a phone
- **WHEN** the user scans the QR code with the mobile app logged in to the same account within 5 minutes
- **THEN** the phone is paired and appears in `cyber remote devices`

#### Scenario: Expired code
- **WHEN** a pairing code is used after 5 minutes
- **THEN** pairing fails with `PairingExpiredError`

### Requirement: Device management
(P3) `cyber remote devices` SHALL list paired Devices with name, platform, last seen time and key fingerprint. `cyber remote revoke <dev_id>` SHALL remove a Device immediately, closing its open channels and rejecting its keys afterwards.

#### Scenario: Lost phone
- **WHEN** the user revokes a lost phone's Device ID
- **THEN** its live connection is closed within 5 seconds and it can no longer attach

### Requirement: Trusted Devices policy
(P3) When org policy sets `policy.remote.require_trusted_devices: true`, only Devices enrolled by an org admin (via the account's device registry) SHALL be allowed to pair or attach. The server SHALL refuse unenrolled Devices with `DeviceNotTrustedError`.

#### Scenario: Unenrolled laptop
- **WHEN** a member tries to attach from a personal laptop not enrolled in the org registry
- **THEN** the attach is refused with `DeviceNotTrustedError`

### Requirement: Attach permission
(P3) Attaching a Device to a Session SHALL be gated by the permission action `remote.attach` with the Device ID as the resource. The default SHALL be `allow` for paired Devices of the same account and `deny` otherwise.

#### Scenario: Deny a device
- **WHEN** `permissions` contains `{ action: "remote.attach", resource: "dev_tablet", effect: "deny" }`
- **THEN** that tablet cannot attach even though it is paired

### Requirement: Remote surface
(P3) A remote Device SHALL be able to list exposed Sessions, read history, admit prompts (steer, queue or hold), interrupt, reply to permission requests and questions, upload attachments (≤ 25 MiB, stored on the local machine and passed as `@` file references), view diffs and snapshots, and monitor and control Workflow Runs, Goals and Loops. It SHALL NOT change config, the account, permission rules or Mode `bypass`, and SHALL NOT open PTYs unless `remote.allow_pty: true`.

#### Scenario: Approve from the phone
- **WHEN** a permission request appears while the user is away
- **THEN** the phone shows it and the user's reply is applied to the local Session

#### Scenario: Bypass from remote refused
- **WHEN** a Device tries to switch the Session to `bypass` mode
- **THEN** the request fails with `ForbiddenError`

### Requirement: Step-up for destructive approvals
(P3) Approving a permission request flagged `destructive` (critical-path removal, force push, `bypass`-only actions) from a remote Device SHALL require an account authentication not older than 15 minutes (`auth_time`). Otherwise the Device SHALL complete an OIDC re-authentication with `prompt=login` before the approval is accepted.

#### Scenario: Stale login
- **WHEN** a Device whose last interactive login was 2 hours ago approves `rm -rf build/` flagged destructive
- **THEN** the Device must re-authenticate before the approval is applied

### Requirement: Synchronized state
(P3) All clients attached to a Session (terminal, web, mobile) SHALL see the same transcript, pending requests and run state within 1 second of a change, and a reply from any client SHALL resolve the request for all of them.

#### Scenario: Answered on the terminal
- **WHEN** a permission is answered in the terminal while the phone shows it
- **THEN** the phone removes the prompt and shows the reply

### Requirement: Reconnect after interruptions
(P3) When the machine sleeps or the network drops, the server SHALL reconnect to the Relay with exponential backoff (1 s to 60 s). Devices SHALL resume their event streams from the last durable sequence, so no durable event is lost.

#### Scenario: Laptop wakes up
- **WHEN** the laptop resumes from sleep after 30 minutes
- **THEN** the server reconnects and the phone receives every durable event committed while it was disconnected

### Requirement: Push notifications
(P3) The server SHALL send push notifications through the Relay to paired mobile Devices for: permission requests, questions, a goal met or failed, a Workflow Run finished, a Session gone idle after more than 2 minutes of work, and an explicit `push_notification` tool call. Push payloads SHALL contain only the Session name and an event kind. Content SHALL be fetched over the encrypted channel.

#### Scenario: Goal met
- **WHEN** a goal completes while the user is away
- **THEN** the phone receives "Goal met in <session name>" without any code in the push payload

### Requirement: Remote session timeout
(P3) An exposed Session with no attached Device for `remote.idle_timeout` (default 24 hours) SHALL stop being exposed. A Device channel idle for 30 minutes SHALL require re-unlock (biometric or app PIN) on mobile.

#### Scenario: Forgotten exposure
- **WHEN** no Device attaches for 24 hours
- **THEN** the Session stops being exposed and the TUI shows that remote access ended

### Requirement: Web client
(P3) The Relay SHALL serve a web client that performs the OIDC login, pairs as a Device using a browser key stored in IndexedDB (non-extractable WebCrypto key), and offers the same remote surface as the mobile apps.

#### Scenario: Browser on another computer
- **WHEN** the user opens the relay URL on another computer and logs in
- **THEN** after pairing they can drive the exposed Sessions

### Requirement: Mobile apps
(P4) The project SHALL ship iOS and Android apps implementing the remote surface, push notifications, QR pairing, biometric unlock and starting new Sessions on a chosen machine or Runner.

#### Scenario: Start work from the phone
- **WHEN** the user creates a Session for `workstation:/repo/app` from the app
- **THEN** the local server creates the Session and it appears in the terminal's session list

### Requirement: Entitlement for hosted relay
(P3) The Cyber Cloud relay SHALL require the `cyber-code` entitlement (any plan) in the account token, and SHALL respond with `EntitlementRequiredError` naming the product when it is missing.

#### Scenario: No entitlement
- **WHEN** an account without the `cyber-code` entitlement connects to the hosted relay
- **THEN** the connection is refused and the CLI explains how to obtain access or self-host a relay

### Requirement: Remote activity log
(P3) Every Device attach, detach, prompt, approval and revoke SHALL be recorded as durable `remote.*` events with the Device ID. `cyber remote log` SHALL print them.

#### Scenario: Audit approvals
- **WHEN** the user runs `cyber remote log --session ses_1`
- **THEN** each remote approval is listed with Device name, time and request
