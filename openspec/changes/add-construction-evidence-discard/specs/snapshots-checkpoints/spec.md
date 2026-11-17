## ADDED Requirements

### Requirement: Owner-only discard of construction evidence
(P1) The system SHALL provide an explicit, owner-only, two-step command that discards unpublished checkout-claim, acknowledgement and cancellation-record construction stages of one Session after a read-only review, and SHALL record a durable audit receipt. Only the authenticated local owner MAY invoke it; delegated input clients, browser leases, peers, relays and model tools SHALL be refused. Automatic maintenance, reclamation and worktree removal SHALL keep refusing while construction evidence remains.

#### Scenario: Review before discard
- **GIVEN** a Session with an interrupted checkout-claim construction stage
- **WHEN** the owner requests a discard review for that stage
- **THEN** the system SHALL report its name, kind, owner activity, native identity, size, modification time, digest, the fences citing it and the Location state, with a confirmation fingerprint
- **AND** the review SHALL NOT modify, lock for writing, adopt or remove anything

#### Scenario: Confirmed discard with an audit receipt
- **GIVEN** a reviewed stage whose activity has no live lifetime holder and no published claim, acknowledgement or cancellation record
- **WHEN** the owner confirms with the unchanged fingerprint and a request ID
- **THEN** the system SHALL durably record a construction-discarded receipt with the stage identities and digest before removing the stage
- **AND** a retry or lookup with the same request ID SHALL return that receipt without removing anything else

#### Scenario: Discard stays fenced
- **WHEN** the stage changed since review, its activity's lifetime lock is held, the Session is busy, a published record exists for the activity, the stage name has no derivable owner, or the caller is not the local owner
- **THEN** the system SHALL refuse and leave every stage, journal and record unchanged

#### Scenario: Evidence of a removed Location
- **GIVEN** a Session whose Location directory was removed and whose construction stages remain
- **WHEN** the owner reviews and confirms their discard
- **THEN** the system SHALL discard only those stages and record the receipt
- **AND** retained restore journals and payload images SHALL stay listed read-only and SHALL NOT be deleted by this command

#### Scenario: One stage per confirmation
- **GIVEN** an activity with more than one construction stage
- **WHEN** the owner confirms a discard
- **THEN** the confirmation SHALL name exactly one reviewed stage and its receipt SHALL cover only that stage
- **AND** the system SHALL NOT offer a bulk discard for an activity or Session

#### Scenario: Failed unlink stays pending
- **GIVEN** a confirmed discard whose stage was moved to the private discarded area but could not be unlinked
- **WHEN** periodic maintenance runs
- **THEN** the system SHALL retry the unlink without a time limit and SHALL NOT drop the entry silently
- **AND** doctor and recovery listings SHALL report it as `discard pending` and its bytes SHALL count toward the snapshot storage cap until removed

#### Scenario: Terminal recovery panel points to the command
- **GIVEN** a Session with construction evidence
- **WHEN** the owner opens the terminal recovery panel
- **THEN** the panel SHALL list the evidence and show the exact CLI discard command
- **AND** the panel SHALL NOT perform the discard itself in this revision
