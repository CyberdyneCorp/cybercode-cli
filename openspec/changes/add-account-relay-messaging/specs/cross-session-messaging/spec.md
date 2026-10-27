## MODIFIED Requirements

### Requirement: Cross-machine transport
(P3) Account-routed messages to remote Sessions through the Relay SHALL travel over that Relay over the end-to-end encrypted machine channel, authenticated with Cyber Account tokens carrying scope `cyber:messaging`. These Relay rules SHALL NOT widen own-machine direct-peer grants. Explicit project-hub conversations SHALL use `project-collaboration` membership, scoped identities and local consent instead; a project binding SHALL NOT grant generic remote Session access. By default only Sessions of the same account (`sub`) SHALL be reachable. Messaging Sessions of other members of a shared org SHALL require `policy.messaging.org_members: true` set by an org admin, plus `allow` rules on both sides.

#### Scenario: Not logged in
- **WHEN** a Session selects account Relay delivery to `build@workstation` and the server has no Cyber Account
- **THEN** the send fails with `AccountRequiredError` and `list_sessions` shows no remote entries

#### Scenario: Cross-account blocked
- **WHEN** a Relay message targets a Session owned by a different `sub` in an org without the policy
- **THEN** it is refused with reason `cross_account_disabled`

#### Scenario: Teammate address
- **WHEN** the org policy allows member messaging, `alex` shares their machine `desk` with the org, both machines are approved, and a Session sends to `review@alex/desk`
- **THEN** the message reaches `alex`'s Session `review` and is held there

#### Scenario: Offline machine
- **WHEN** a Session sends to `build@workstation` while `workstation` is offline
- **THEN** the send fails with `MachineUnreachableError { machine: "workstation" }`, nothing is queued, and the sender records `failed / unreachable`

#### Scenario: Peer and machine share a name
- **WHEN** a configured peer and an approved own machine are both named `desk` and a Session sends to `review@desk`
- **THEN** the send fails with `AmbiguousAddressError` listing `peer:desk` and `machine:desk`

## ADDED Requirements

### Requirement: Machine trust approval
(P3) A machine that appears in the account or org roster SHALL be `pending` until the user approves it locally, and no listing, message or watch SHALL flow until both machines have approved each other. Both machines SHALL display the same six-word code, derived from the SHA-256 of their two public keys in sorted order, for the user to compare. Approval SHALL be stored per machine ID and public key. A known machine ID that presents a different public key SHALL return to `pending` with a warning naming the previous fingerprint. `cyber machines forget` SHALL delete the trust and close open channels immediately. `cyber machines reject` SHALL keep a tombstone so the machine stops prompting.

#### Scenario: New machine on the account
- **WHEN** the user enables the Relay on a new laptop that is signed in to the same account as their desktop
- **THEN** each machine shows the other as pending with the same six-word code, and the laptop's Sessions become reachable from the desktop only after both sides approve

#### Scenario: Injected machine
- **WHEN** the Relay lists a machine the user never set up
- **THEN** it stays pending, nothing is sent to it, and the user can reject it

#### Scenario: Key change
- **WHEN** an approved machine reconnects with a different public key
- **THEN** it becomes pending again and the warning shows the old and new fingerprints

### Requirement: Relay message delivery
(P3) A message sent over the Relay SHALL go through the target's normal pipeline: size, rate and chain limits, inbound rules, untrusted rendering and durable records in both Sessions. The receiver SHALL name the sender from the channel identity, never from fields in the frame. The default inbound policy for machines of the same account SHALL be `hold`, and a `messaging.inbound.deliver` pattern naming the machine (`*@laptop`) SHALL deliver without holding. Messages from another account SHALL always be held, whatever the inbound patterns and Session overrides say. Releasing such a message SHALL recheck that the machine is still approved and that the org policy still allows it. Outcomes of held messages SHALL reach the sender asynchronously as `released`, `dropped` or `expired`, as for direct peers.

#### Scenario: Own machine held by default
- **WHEN** a Session on `laptop` sends to `review@desk` on the same account and `desk` has no deliver pattern for it
- **THEN** `desk` holds the message and the sender's outcome is `held`, later `released` when the user releases it

#### Scenario: Teammate messages cannot be auto-delivered
- **WHEN** `desk` has `messaging.inbound.deliver: ["*"]` and a message arrives from `ci@sam/box`, a machine of another account
- **THEN** the message is still held

### Requirement: Relay reachable listing
(P3) `list_sessions`, `GET /api/v1/messaging/sessions` and `cyber messages list --reachable` SHALL include, alongside peers, the root Sessions of each online approved machine that would accept the caller's messages. There SHALL be at most 50 per machine, as `<session>@<machine>` or `<session>@<member>/<machine>`, with `kind: relay`, with a 5-second timeout and a 15-second cache. An approved machine that is offline SHALL appear as a single `*@<machine>` entry with state `offline`. Locations SHALL show the workspace name only, unless the receiver sets `peers.share_paths`. `watch_session` on a Relay address SHALL poll the target's status every 15 seconds and expire after 12 hours.

#### Scenario: Two own machines
- **WHEN** the desktop and the laptop are approved and the laptop lists reachable Sessions
- **THEN** the desktop's reachable root Sessions appear as `<title>@desktop` with `kind: relay`

#### Scenario: Machine asleep
- **WHEN** an approved machine is offline
- **THEN** the listing shows one entry `*@<machine>` with state `offline` instead of its Sessions
