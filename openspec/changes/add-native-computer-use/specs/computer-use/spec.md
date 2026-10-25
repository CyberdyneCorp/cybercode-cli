## ADDED Requirements

### Requirement: Native computer capability and consent
(P2) Native computer use SHALL be experimental and off by default, enableable only through trusted global configuration. Its first release SHALL support macOS 14+ and return explicit unsupported-platform/version results elsewhere. Capture/input SHALL require current OS Screen Recording/Accessibility authorization as applicable and an explicit authenticated owner grant bound to one Session and chosen app/window/display. Grants SHALL default to 300 seconds, be capped at 900 seconds, require attached human presence and never survive restart or inherit to child/hub agents.

#### Scenario: Repository attempts to enable host input
- **WHEN** project configuration or model content requests feature enablement or a control grant
- **THEN** it cannot arm native capture/input or change global feature state

#### Scenario: Unsupported host
- **WHEN** the feature is requested on Linux, Windows or an unsupported macOS version
- **THEN** the server reports the unsupported capability without shell fallback or native side effects

### Requirement: Frame-bound native input
(P2) The computer tool SHALL expose status, screenshot, move, click, drag, scroll, type, key and bounded wait actions. A capture SHALL return server-owned frame identity, grant/target generation, timestamp, dimensions and coordinate transform. Input SHALL require a frame at most 10 seconds old, valid grant/OS consent, matching Session, target/focus/geometry and in-bounds coordinates. Plan mode SHALL forbid input. Existing saved/bypass permissions SHALL NOT arm or widen host control. Default limits SHALL be 10 input actions per second, 300 per grant, 5-second action/wait duration and 16 KiB of typed text.

#### Scenario: Window moves after capture
- **WHEN** an input action uses a frame whose target geometry or foreground identity has changed
- **THEN** the action refuses before injecting events and requires a fresh capture

#### Scenario: Edit text in the granted test application
- **WHEN** an armed permitted Session with a fresh matching frame submits bounded Unicode text/key input
- **THEN** the native adapter edits only within the reviewed current target without clipboard scraping or shell execution

### Requirement: Native stop and unknown outcomes
(P2) At most one Session SHALL hold host input authority. Owner Stop, Session interrupt, human-presence loss, grant expiry, OS revocation or restart SHALL fence later input independently of the next model Turn. Native actions SHALL persist intent and settlement by operation identity. An interrupted action that may have delivered input SHALL remain unknown and SHALL NOT be automatically replayed. Cleanup SHALL release pressed keys/buttons where possible and report uncertainty when release cannot be established.

#### Scenario: Stop during model execution
- **WHEN** an owner requests Stop while the model or another tool is busy
- **THEN** the native control generation is revoked without waiting for that Turn and no later action under it is dispatched

#### Scenario: Crash after a click may have occurred
- **WHEN** recovery finds an admitted click without an authoritative settlement
- **THEN** it reports an unknown outcome and requires reconciliation/fresh observation rather than clicking again

### Requirement: Private computer evidence
(P2) Captures SHALL remain in owner-protected Session/target-bound artifact storage, default to one-day retention with zero disabling time-based pruning, and use authenticated preview reads. Maximum default capture edge SHALL be 1920 pixels and encoded size 8 MiB with retained coordinate transforms. Audit/export records SHALL omit raw typed text while retaining length/digest, target/frame references and permission/settlement state. On-screen content SHALL be treated as untrusted data, not instructions authorizing a new action. No clipboard, credential-store, audio/video or unrelated-display capture SHALL occur implicitly.

#### Scenario: Screen contains an instruction to approve a tool
- **WHEN** a captured application displays text requesting permission or policy changes
- **THEN** that content grants no authority to arm, renew or widen computer control or approve another tool

#### Scenario: Inspect a text-entry audit record
- **WHEN** an owner exports or inspects durable records for a type action
- **THEN** the raw typed text is absent and the report states that authorized screenshots/model input can still contain sensitive visible content

#### Scenario: Session deletion with retention disabled
- **GIVEN** a Session owns private captures with time-based retention disabled or a capture worker is finishing
- **WHEN** the Session is deleted
- **THEN** its frames become unavailable atomically with deletion, file cleanup is attempted and retried after failures, late workers cannot recreate capture artifacts, and minimal native unknown-outcome evidence remains available for reconciliation without screenshot bytes
