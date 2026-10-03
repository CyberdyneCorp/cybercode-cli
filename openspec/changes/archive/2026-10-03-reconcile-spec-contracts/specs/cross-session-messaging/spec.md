## MODIFIED Requirements

### Requirement: Messaging API
(P1) The server SHALL expose `GET /api/v1/messaging/sessions`, `POST /api/v1/messaging/send`, `GET /api/v1/sessions/:sessionID/messaging` (history) and `POST /api/v1/messaging/watch`, so clients and SDKs can message Sessions without going through a model. Held messages SHALL be released or dropped through the Session inbox routes defined by `server-api` (`POST /api/v1/sessions/:sessionID/inbox/:messageID/release|drop`); messaging SHALL NOT define separate release routes.

#### Scenario: Human message from the SDK
- **WHEN** a script posts to `/api/v1/messaging/send` targeting a running Session
- **THEN** the message is delivered under the same inbound controls, with origin `client`

#### Scenario: Release a held message
- **WHEN** a client posts to `/api/v1/sessions/ses_1/inbox/msgx_7/release`
- **THEN** the held cross-session message is promoted at the next Safe Boundary and `messaging.message.released.1` is emitted to the sender
