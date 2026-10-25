## ADDED Requirements

### Requirement: Messaging-only Relay connection
(P3) With `messaging.relay.enabled: true` (accepted only from user config and managed policy) and a logged-in Cyber Account, the server SHALL open the outbound Relay connection to `messaging.relay.url`, overridden by `policy.remote.relay_url`, for cross-machine messaging without exposing any Session to Devices. Remote control SHALL still require `/remote` or `cyber remote enable`. The connection SHALL ping every 20 seconds, reconnect with jittered backoff between 1 second and 5 minutes, close on logout, and be disabled by the managed policy `policy.messaging.relay: false`. `cyber doctor` SHALL report the Relay state, machine ID, key fingerprint and the counts of approved and pending machines.

#### Scenario: Messaging without remote control
- **WHEN** a user enables `messaging.relay` but never runs `/remote`
- **THEN** other approved machines can message the user's Sessions, and no paired Device can see or attach to them

#### Scenario: Project config cannot enable it
- **WHEN** a checkout's `.cyber/config.json` sets `messaging.relay.enabled: true`
- **THEN** the configuration load fails naming that path

### Requirement: Relay machine roster
(P3) The Relay SHALL keep, for each account, a roster of machine IDs, public keys, online state and last-seen time, and SHALL push roster changes to that account's connected machines. It SHALL include the machines of other members of an org only when the org policy `policy.messaging.org_members` is true and the member set `messaging.relay.share_with_org: true`, labelled with the member's verified `preferred_username`, or their `sub` when no username is set. The Relay SHALL refuse an announce whose machine name is already used in the account with `MachineNameTakenError`. It SHALL limit each machine to 120 frames per minute and 64 KiB per frame, and each account to 32 connections. `cyber relay serve` SHALL provide this roster and routing for self-hosting.

#### Scenario: Teammate not sharing
- **WHEN** the org policy allows member messaging but `alex` has not set `share_with_org`
- **THEN** no machine of `alex` appears in other members' rosters
