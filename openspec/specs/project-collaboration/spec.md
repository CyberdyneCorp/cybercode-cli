# project-collaboration Specification

## Purpose
Bind project collaboration actions to verified accounts, current membership and repository authorization.

## Requirements

### Requirement: Local credential and replay identity binding
(P3) A trusted local connector SHALL obtain a human access token and account identity from one verified credential snapshot. Public hub discovery SHALL omit Authorization; before sending a token to a bound hub, the connector SHALL compare its public issuer and hub identity with that snapshot and binding. An optional owner `expected_actor` containing issuer and subject SHALL reject a stale displayed account with `409 AccountChangedError` and SHALL NOT select the mutation author or cross into hub payloads. A queued human mutation SHALL bind the observed member ID, membership generation and project policy revision; replay SHALL reject missing or changed scope rather than dispatch an old intent under a later same-subject admission. Logout revocation SHALL preserve the verified credential identity and SHALL NOT send its token to a hub of another issuer.

#### Scenario: Account changes after browser review
- **WHEN** an owner mutation carries a displayed actor that differs from the connector's current credential snapshot
- **THEN** it fails before enqueue or hub mutation and requires fresh review

#### Scenario: Unobserved revoke and rejoin
- **WHEN** an unattempted queued mutation belongs to an old member generation and the same subject rejoins before reconnecting
- **THEN** replay records a local rejection and does not send the old mutation under the new generation

### Requirement: Fresh contribution-reference authorization
(P3) A hub SHALL expose a human-only read operation that freshly validates the invoking identity, current project write membership, repository identity, optional same-project task, and optional current published agent and its freshly validated owner. This operation SHALL NOT enroll a member or modify admission state. The local connector SHALL compare the response project/repository with its confirmed checkout binding and current remote. Unavailable live identity validation SHALL fail closed.

#### Scenario: Positive cache followed by org revocation
- **WHEN** a human or referenced agent owner loses the required organization after a positive cached read
- **THEN** contribution-reference authorization performs fresh validation and refuses the revoked identity
