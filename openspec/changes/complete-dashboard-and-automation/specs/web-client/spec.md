## ADDED Requirements

### Requirement: Browser project collaboration
(P3) The owner-authenticated browser SHALL expose People, Conversation, shared Tasks and a held-message inbox through the local collaboration API for the selected Location and bound project. It SHALL identify verified human owners separately from published agents, respect current visibility and revision/claim fences, and show disabled, unauthenticated, unbound, queued and stale states explicitly. Opening a checkout SHALL NOT enable collaboration or publish an agent.

#### Scenario: Teammates coordinate in the browser
- **WHEN** two authorized members use browsers connected to their own local servers bound to one hub project
- **THEN** each can inspect permitted participants, send permitted project/direct messages and create or claim shared tasks without exposing private local transcripts or remote execution authority

#### Scenario: Receiver releases a held message
- **WHEN** a receiver inspects a held agent message and requests release
- **THEN** the browser binds the action to its exact message, project and target Session and the server rechecks current authorization and receiver policy before admission
- **AND** a revoked sender or stale action is refused without admitting another message

#### Scenario: Mutation acknowledgement is lost
- **WHEN** a task claim or message mutation has an uncertain transport outcome
- **THEN** the browser retains the operation key and reconciles its receipt instead of creating a new mutation or reporting success without acknowledgement

### Requirement: Browser settings editing
(P1) The browser SHALL provide schema-backed editing of supported configuration with explicit project/global scope, source attribution, managed-policy locks, validation errors and a reviewed change preview using the configuration API. Redaction placeholders SHALL NOT be written as secrets. Secret replacement/clear actions SHALL be explicit and drafts SHALL remain in page memory. Global-only settings SHALL NOT be offered as project writes; restart-required changes SHALL be explained before applying them.

#### Scenario: Edit a project setting
- **WHEN** an owner reviews and applies a valid project-scoped change
- **THEN** the browser uses the configuration API and displays the returned effective value and source without rewriting unrelated settings or comments

#### Scenario: Preserve a hidden secret
- **WHEN** the owner changes a non-secret value while existing credentials are redacted
- **THEN** the browser omits untouched credential fields and does not persist or submit the redaction placeholder

### Requirement: Browser attachment upload and previews
(P1) The browser SHALL support image/PDF attachments within existing server size, media and model-modality limits. Previews SHALL use authenticated reads restricted to blobs referenced by the selected Session, remain inert, fetch no arbitrary transcript URL and expose no arbitrary local file path. Unsupported active content SHALL be refused; temporary preview resources SHALL be released on logout or Session switch.

#### Scenario: Preview an admitted image
- **WHEN** an owner opens a Session containing a stored supported image attachment
- **THEN** its preview is loaded through the authenticated reference-bound API without placing owner credentials in a URL or browser storage

#### Scenario: Refuse a fabricated blob reference
- **WHEN** a preview request names a blob not referenced by its Session
- **THEN** the server denies the read without disclosing the blob or searching an arbitrary filesystem path

### Requirement: Browser PTY Job terminal
(P1) Where the server supports PTY Jobs, the browser SHALL offer interactive attachment to an existing running Job with bounded output replay, input and resize. It SHALL obtain a short-lived single-use Job-bound ticket through owner authentication and SHALL NOT place owner credentials in a WebSocket URL. Terminal output SHALL NOT automatically invoke clipboard writes, fetch remote resources or execute links. Reconnect SHALL NOT resend uncertain input; disconnect SHALL NOT stop a Job unless the owner explicitly requests stop.

#### Scenario: Resume terminal output after disconnect
- **WHEN** a connected terminal loses its socket and reconnects to the same running Job
- **THEN** it obtains a fresh ticket, resumes bounded output from its cursor and does not replay previously submitted input

#### Scenario: Unsupported platform
- **WHEN** the connected server does not support PTY Jobs
- **THEN** terminal controls are unavailable with a capability explanation rather than starting an unrestricted alternate shell
