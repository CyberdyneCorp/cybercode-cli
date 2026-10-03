# session-sharing Specification

## Purpose
Session sharing publishes a Session, or a generated report, as a page others can open. It keeps the page in sync as the Session changes, scopes access to its owner, their organization, or anyone with the link, and lets shared Sessions be imported back locally. It builds on OpenCode v1's share/sync/import design and Claude Code's Artifacts and session sharing. It adds a mandatory redaction pipeline, organization-scoped visibility from Cyber Account claims, and a self-hostable share service.

## Requirements

### Requirement: Share modes
(P3) The system SHALL read config `share` with values `manual` (default), `auto` or `disabled`. In `auto` mode, every new root Session SHALL be shared in the background, and a failure SHALL NOT fail Session creation. Child Sessions (subagents, workflow agents) SHALL never be auto-shared. `disabled` SHALL make every share operation fail with `Sharing is disabled`.

#### Scenario: Auto-share skips subagents
- **WHEN** `share` is `auto` and a Session spawns two subagents
- **THEN** only the root Session is shared

### Requirement: Account requirement
(P3) The system SHALL require a signed-in Cyber Account whose token carries the `cyber:share` scope to create or update shares. Without one, share operations SHALL fail with `Sign in with cyber login to share sessions`, and SHALL never fall back to anonymous uploads.

#### Scenario: Not signed in
- **WHEN** a signed-out user runs `/share`
- **THEN** the TUI shows the sign-in message and nothing is uploaded

### Requirement: Share service endpoint
(P3) The system SHALL send share requests to `share.url` when set, and otherwise to the Cyber Cloud share service. Requests SHALL be authenticated with `Authorization: Bearer <Cyber Account access token>`. The system SHALL provide `cyber share serve`, a self-hostable share service that validates tokens against the configured CyberdyneAuth issuer through OIDC discovery and JWKS, never through hard-coded keys.

#### Scenario: Self-hosted service
- **WHEN** `share.url` is `https://share.corp.example` and the user shares a Session
- **THEN** the upload goes to that host, which verifies the token's `iss` and signature from the issuer's discovery document

### Requirement: Visibility levels
(P3) The system SHALL support three visibility levels per share:
- `private`: only the owner `sub`
- `org`: members of a chosen organization from the token's `orgs` claim
- `public`: anyone with the link

The default SHALL be `share.default_visibility` (default `private`). Changing visibility SHALL take effect within 5 seconds, and narrowing it SHALL invalidate cached public renders.

#### Scenario: Org-only share
- **WHEN** the owner shares with visibility `org` for organization `acme`
- **THEN** a viewer whose token's `orgs` includes `acme` can open the page, and a viewer outside `acme` receives 404

#### Scenario: Missing orgs claim is not all orgs
- **WHEN** a viewer's token has no `orgs` claim
- **THEN** org-visibility shares are denied to that viewer

### Requirement: Redaction before upload
(P3) The system SHALL pass every uploaded item through a redaction pipeline before it leaves the machine:
- secrets matched by the built-in detectors (API keys, tokens, private keys, connection strings) and by user patterns in `share.redact.patterns` are replaced with `[redacted:secret]`
- values of environment variables named in `share.redact.env` are redacted
- with `share.redact.paths: true`, absolute home paths are replaced with `~`

Redaction counts SHALL be shown to the user on the first share of a Session.

#### Scenario: API key in tool output
- **WHEN** a bash output containing `sk-live-abc123...` is synced
- **THEN** the uploaded part contains `[redacted:secret]` and the local Session is unchanged

### Requirement: Manual share and confirmation
(P3) The system SHALL share a Session on `/share` (TUI), `cyber session share <id>`, exec `--share`, or `POST /api/v1/sessions/:id/share` with optional `visibility`. Before the first share in a project, the TUI SHALL confirm with the visibility and redaction summary. The confirmation SHALL be remembered per project. The share URL SHALL be stored on the Session and copied to the clipboard.

#### Scenario: Copy existing link
- **WHEN** the Session is already shared and the user runs `/share`
- **THEN** the existing URL is copied without creating a new share

### Requirement: Initial and incremental sync
(P3) The system SHALL, after creating a share, upload in the background the Session info, all messages and parts, per-turn diffs, and the models used. Afterwards it SHALL coalesce changes from Session events per item key, and flush them about 1 second after the first queued change. Sync failures SHALL be retried with exponential backoff (maximum 5 attempts) and SHALL NOT affect the Session.

#### Scenario: Debounced updates
- **WHEN** 40 part deltas arrive within 1 second for a shared Session
- **THEN** a single sync request carries the final state of the changed parts

### Requirement: Unshare and deletion
(P3) The system SHALL unshare on `/unshare`, `cyber session unshare <id>`, or `DELETE /api/v1/sessions/:id/share`. It SHALL delete the remote copy, clear the Session's share field, and remove the local share record. Deleting a shared Session SHALL delete its remote share. A remote deletion failure SHALL be retried and surfaced as a warning toast.

#### Scenario: Delete removes remote copy
- **WHEN** the user deletes a shared Session
- **THEN** the share service receives a delete for that share and the URL returns 404 afterwards

### Requirement: Retention
(P3) The share service SHALL retain shares until they are unshared, the Session is deleted, or the owner's account is deleted. Organizations MAY set `share.retention_days` through org policy. Shares older than the retention period SHALL be deleted daily, and owners SHALL be notified 7 days before.

#### Scenario: Org retention
- **WHEN** org policy sets `share.retention_days: 30`
- **THEN** a 31-day-old share from that org is deleted by the daily job

### Requirement: Kill switch and policy
(P3) The system SHALL disable all share network activity when `CYBER_DISABLE_SHARE` is `1` or `true`, or when org policy sets `share: "disabled"`. In both cases share commands SHALL be hidden in the TUI, and API calls SHALL fail with 403 `SharingDisabledError`.

#### Scenario: Policy disables sharing
- **WHEN** the active org policy sets `share: "disabled"` and a user's project config sets `share: "auto"`
- **THEN** no Session is shared and `/share` is not offered

### Requirement: Export with sanitization
(P1) The system SHALL provide `cyber export [session]`, which writes the Session and its messages and parts as JSON to stdout, and `--sanitize`, which replaces transcript text, paths, URLs, tool outputs and metadata with `[redacted:<kind>:<id>]` markers. `--format markdown` SHALL produce a readable transcript.

#### Scenario: Sanitized bug report
- **WHEN** a user runs `cyber export --sanitize ses_123 > report.json`
- **THEN** the file keeps the message and tool-call structure, but no original text, paths or outputs

### Requirement: Import
(P1) The system SHALL provide `cyber import <file|share-url>`. It SHALL accept an export file or a share URL, fetch share data with the account token when the share is not public, re-home the Session to the current Location, insert messages without overwriting existing rows, and print `Imported session: <id>`. An invalid URL SHALL fail with `Invalid share URL`.

#### Scenario: Import an org share
- **WHEN** a teammate in the same org runs `cyber import https://share.cyber.dev/s/abc`
- **THEN** the Session is imported into their current project and can be resumed

### Requirement: Share artifacts
(P4) The system SHALL let the agent or the user publish a generated HTML or Markdown file as a share artifact, using `/publish <file>` or the `publish_artifact` tool, which requires the `share.publish` permission. Artifacts SHALL use the same visibility levels, redaction and account requirement as Session shares. Republishing from the same Session SHALL update the artifact in place at the same URL, and the service SHALL keep the last 20 versions.

#### Scenario: Republish keeps the URL
- **WHEN** the agent republishes `report.html` after edits
- **THEN** the artifact URL is unchanged and shows the new version

### Requirement: Comments on shares
(P4) The share service SHALL allow viewers with access to comment on messages of a shared Session or on artifact sections. The owner's server SHALL receive new comments as a Channel event when the owner opted in with `share.comments_to_session: true`, so the agent can answer them.

#### Scenario: Comment reaches the session
- **WHEN** a teammate comments `why this approach?` on a shared message and the owner opted in
- **THEN** the comment is admitted to the owner's Session with `delivery: queue`, attributed to the commenter
