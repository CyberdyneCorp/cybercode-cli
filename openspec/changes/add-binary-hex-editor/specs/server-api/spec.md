## ADDED Requirements

### Requirement: Binary owner file routes
(P1) The owner API SHALL add document open, revision-scoped bytes/search, patch/history/review/save/restore, operation lookup and explicit close routes under `/api/v1/files/binary/documents` in the existing `fs` group. All calls SHALL require normal owner authentication, Origin rules, exact Session/Location scope, regular-file identity, current permissions and bounded typed payloads. Document IDs SHALL NOT act as bearer grants. Bytes SHALL use bounded base64 bodies, offsets/lengths SHALL be exact decimal strings, and content SHALL NOT enter URLs, logs or caches. Existing text browsing contracts SHALL remain compatible. SDK helpers SHALL preserve byte/offset precision and retained operation identity.

#### Scenario: Reuse another document ID
- **WHEN** a caller submits a document ID under a different Session/Location without its bound authority
- **THEN** the route refuses before returning byte content or modifying its draft
