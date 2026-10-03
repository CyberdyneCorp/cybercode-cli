## MODIFIED Requirements

### Requirement: Push notifications
(P3) The server SHALL send push notifications through the Relay to paired mobile Devices for: permission requests, questions, a goal met or failed, a Workflow Run finished, a Session gone idle after more than 2 minutes of work, and an explicit `notify` tool call (`background-tasks`). Push payloads SHALL contain only the Session name and an event kind. Content SHALL be fetched over the encrypted channel.

#### Scenario: Goal met
- **WHEN** a goal completes while the user is away
- **THEN** the phone receives "Goal met in <session name>" without any code in the push payload
