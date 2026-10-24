## ADDED Requirements

### Requirement: Integration queue workflow surfaces
(P2) `cyber workflows integrations submit|list|show|run|cancel|report|publish` and `/api/v1/workflows/integrations` SHALL expose the verified integration queue through existing workflow ownership, Location, permission, idempotency and budget boundaries. SDK and browser clients SHALL show manifests, progress, conflicts, host evidence, independent review, target staleness and explicit publication outcomes. Local owner-selected batches SHALL work without an account; authenticated hub-linked actions SHALL preserve current membership requirements.

#### Scenario: Owner runs a local integration batch
- **WHEN** an owner submits local commit contributions and admitted checks without a Cyber Account
- **THEN** the local queue can verify them and present evidence without claiming hub membership or publishing automatically

#### Scenario: Browser retries a cancellation
- **WHEN** a browser loses acknowledgement of cancellation
- **THEN** it retries the same revision-bound operation key without cancelling a newer attempt
