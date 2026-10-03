## MODIFIED Requirements

### Requirement: Hold delivery approval
(P3) Events admitted with `delivery: hold` SHALL appear in the TUI, web and mobile clients as pending inbound items that the user can release or drop through the Session inbox routes defined by `server-api` (`POST /api/v1/sessions/:sessionID/inbox/:messageID/release|drop`). Unreleased held events SHALL expire after 24 hours.

#### Scenario: Review before acting
- **WHEN** a `hold` channel receives a production alert
- **THEN** it waits as a pending item until the user releases it into the Session
