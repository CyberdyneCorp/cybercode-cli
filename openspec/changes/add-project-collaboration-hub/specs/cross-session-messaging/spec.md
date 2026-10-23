## MODIFIED Requirements

### Requirement: Cross-machine transport
(P3) Account-routed messages to remote Sessions through the Relay SHALL travel over that Relay over the end-to-end encrypted machine channel, authenticated with Cyber Account tokens carrying scope `cyber:messaging`. These Relay rules SHALL NOT widen own-machine direct-peer grants. Explicit project-hub conversations SHALL use `project-collaboration` membership, scoped identities and local consent instead; a project binding SHALL NOT grant generic remote Session access. By default only Sessions of the same account (`sub`) SHALL be reachable. Messaging Sessions of other members of a shared org SHALL require `policy.messaging.org_members: true` set by an org admin, plus `allow` rules on both sides.

#### Scenario: Not logged in
- **WHEN** a Session selects account Relay delivery to `build@workstation` and the server has no Cyber Account
- **THEN** the send fails with `AccountRequiredError` and `list_sessions` shows no remote entries

#### Scenario: Cross-account blocked
- **WHEN** a Relay message targets a Session owned by a different `sub` in an org without the policy
- **THEN** it is refused with reason `cross_account_disabled`


## ADDED Requirements

### Requirement: Project collaboration message integration
(P3) Project-hub agent messages SHALL enter through a separately identified collaboration transport, with hub/project/message and authenticated human/agent identities retained in durable inbox records. Existing local and direct-peer addressing SHALL keep their semantics. Model-supplied text or sender fields SHALL NOT manufacture project identity, membership or a human author. The local connector SHALL enforce `project-collaboration` authorization and revalidate held-message release, while preserving messaging disable switches, non-interactive refusal, untrusted-input framing, chain/recursion limits and local permissions. Human-to-human hub conversation SHALL not require a local model Turn.

#### Scenario: Local messaging disabled
- **WHEN** the receiving runtime has messaging disabled and a hub targets one of its published agents
- **THEN** no model prompt is admitted and the hub receives an explicit refusal rather than a false delivery claim

#### Scenario: Existing own-machine peer
- **WHEN** an existing authorized direct-peer send is performed without a Cyber Account
- **THEN** it retains its current behavior and acquires no cross-person project authority
