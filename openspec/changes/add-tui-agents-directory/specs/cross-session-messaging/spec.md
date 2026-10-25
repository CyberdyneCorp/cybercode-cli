## ADDED Requirements

### Requirement: Reachable session provenance
(P1) In the reachable Session listing (`GET /api/v1/messaging/sessions`, `list_sessions` and `cyber messages list --reachable`):

- a member of the caller's team SHALL be listed with `kind: "teammate"`. Its `address` SHALL be its member name when that name resolves to it from the caller.
- an entry for a Session that a workflow run created SHALL include `workflowRunId`.

Other entries SHALL be unchanged, and both additions SHALL be optional for readers, so an older client still reads the listing.

#### Scenario: Teammate is tagged
- **WHEN** a team lead lists reachable Sessions while its teammate `api` runs
- **THEN** the teammate's entry has `kind: "teammate"` and `address: "api"`, and sending to `api` reaches it

#### Scenario: Workflow agent is tagged
- **WHEN** a Session that owns a running workflow run lists reachable Sessions and the listing includes the run's agent Sessions
- **THEN** each of those entries carries a `workflowRunId` equal to the run's ID, and entries for Sessions no workflow run created carry none
