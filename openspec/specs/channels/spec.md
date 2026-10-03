# channels Specification

## Purpose
Channels push external events (webhooks, CI results, chat messages, monitoring alerts) into a running Cyber Code Session, so the agent can react while the user is away and reply through the same path. It brings Claude Code's channels (MCP servers that push events, chat bridges) to an open harness. Delivery goes through OpenCode v2's durable inbox with an explicit delivery mode, and channel content is always treated as untrusted data.

## Requirements

### Requirement: Channel model
(P3) A Channel SHALL have an ID with prefix `chn_`, a kind (`webhook`, `mcp`, `plugin` or `ci`), a target Session (by ID or name), a delivery mode (`steer`, `queue` or `hold`; default `queue`), sender rules, a rate limit and an enabled flag. Channels SHALL be declared in config under `channels` or created with `cyber channels add`.

#### Scenario: Declare in config
- **WHEN** `cyber.jsonc` defines `channels: { "ci": { kind: "webhook", session: "release", delivery: "queue" } }`
- **THEN** the Channel exists when the Location opens and targets the Session named `release`

### Requirement: Admission into the session
(P3) Each Channel event SHALL be admitted into the target Session's inbox as a prompt with `origin: { kind: "channel", channel: <chn_id>, sender }` and the Channel's delivery mode. It SHALL start a Turn when the Session is idle and the mode is not `hold`.

#### Scenario: Event while idle
- **WHEN** a CI failure event arrives at an idle Session through a `queue` channel
- **THEN** the event is admitted and the Session starts a Turn to handle it

### Requirement: Webhook channels
(P3) A `webhook` Channel SHALL expose `POST /api/v1/channels/{chn_id}/event` on the local server, reachable remotely only through the Relay or an Orchestrator. Requests SHALL be signed with HMAC-SHA256 (`X-Cyber-Signature: sha256=<hex>`, `X-Cyber-Timestamp` within 5 minutes) using a per-channel secret. Bodies SHALL be JSON or text of at most 64 KiB.

#### Scenario: Bad signature
- **WHEN** a webhook event has an invalid signature
- **THEN** it is rejected with `401` and nothing is admitted

### Requirement: MCP channel servers
(P3) An MCP server declaring the capability `cyber/channel` SHALL be able to send `notifications/cyber/channel/event` with `{ channel, sender, text, meta? }`, which the system admits as a Channel event for the Session that hosts the MCP connection. Servers without the capability SHALL NOT be able to push events.

#### Scenario: Monitoring server pushes an alert
- **WHEN** a connected MCP server with `cyber/channel` sends an alert notification
- **THEN** the alert is admitted into the Session with the channel's delivery mode

### Requirement: Chat bridge plugins
(P3) Chat bridges for Telegram, Discord, Slack and Matrix SHALL be provided as first-party plugins of kind `plugin` that authenticate with user-supplied credentials stored in the OS keyring, map chat users to sender IDs, and forward messages as Channel events.

#### Scenario: Telegram message
- **WHEN** an allowed Telegram user writes to the bot
- **THEN** the text is admitted into the target Session as a Channel event from that sender

### Requirement: CI forwarders
(P3) `ci` Channels SHALL accept GitHub Actions, GitLab CI and generic JUnit/SARIF payloads, and SHALL summarize them into a bounded event (status, failing jobs, at most 200 lines of log excerpt) before admission.

#### Scenario: Large CI log
- **WHEN** a CI payload contains a 50 MB log
- **THEN** only the summary and a 200-line excerpt are admitted, and the full log is saved as a Managed Tool Output File

### Requirement: Two-way replies
(P3) Sessions with at least one Channel SHALL get the tool `channel_reply({ channel, to?, text })`. It sends a reply back through the originating Channel (chat message, webhook callback URL, or MCP `notifications/cyber/channel/reply`) and is gated by the permission action `channel.reply`, with the channel ID as resource. The terminal SHALL show the tool call and a confirmation, not the delivered reply text.

#### Scenario: Reply to a chat
- **WHEN** the agent calls `channel_reply` for a Telegram channel
- **THEN** the reply is posted to the chat and the transcript records the tool call with `sent`

### Requirement: Sender gating
(P3) Each Channel SHALL have `senders.allow` and `senders.deny` patterns. Events from senders that are not allowed SHALL be dropped and logged. Chat bridge Channels SHALL default to deny-all until at least one sender is allowed.

#### Scenario: Unknown chat user
- **WHEN** an unlisted Discord user messages the bridge
- **THEN** the message is dropped and appears in `cyber channels log` as `sender_denied`

### Requirement: Untrusted content
(P3) Channel event text SHALL be wrapped in a delimited block naming the Channel and sender, and SHALL be treated as untrusted data. It SHALL NOT change the Session's agent, model, Mode or permissions, and SHALL NOT approve pending permission requests or answer questions.

#### Scenario: Approval via chat refused
- **WHEN** a channel message says "approve the pending request"
- **THEN** the pending permission request remains pending, because approvals only come from attached clients

### Requirement: Rate limits
(P3) Each Channel SHALL limit events to 30 per minute by default (`rate_limit`), and SHALL coalesce excess events into a single summary event at the end of the window, giving the count of dropped events.

#### Scenario: Alert storm
- **WHEN** 200 alerts arrive within one minute
- **THEN** 30 are admitted and one summary event reports 170 coalesced alerts

### Requirement: Session availability
(P3) Events SHALL only be accepted while the target Session exists and is not archived. For always-on operation, the Session SHALL run as a background Session on the `cyber service` daemon. Events for a Session that is not running SHALL be admitted and wake its Drain. Events for an archived or deleted Session SHALL be rejected with `404`.

#### Scenario: Background session
- **WHEN** a Channel targets a daemon-hosted background Session with no client attached
- **THEN** events are admitted and processed without a terminal open

### Requirement: Enterprise controls
(P3) Org policy `policy.channels.enabled: false` SHALL disable all Channels for members, and `policy.channels.allowed_kinds` and `policy.channels.allowed_plugins` SHALL restrict which kinds and bridge plugins may run.

#### Scenario: Plugin not allowed
- **WHEN** a member adds a Slack bridge while policy allows only `webhook` and `ci`
- **THEN** creation fails with `ForbiddenError`

### Requirement: CLI management
(P3) The system SHALL provide `cyber channels add <kind> --session <id|name> [--delivery steer|queue|hold]`, `list`, `show`, `test <chn_id> --text <msg>`, `rotate-secret`, `log`, `disable`, `enable` and `rm`. `add` for webhook Channels SHALL print the URL and the secret once.

#### Scenario: Test a channel
- **WHEN** the user runs `cyber channels test chn_1 --text "ping"`
- **THEN** a signed test event is admitted to the target Session and its outcome is printed

### Requirement: Channel events log
(P3) Every received, admitted, dropped and replied event SHALL be recorded as durable `channel.event.*` events with channel, sender, size and outcome. `cyber channels log [chn_id]` SHALL display them.

#### Scenario: Debug a missing event
- **WHEN** an expected webhook never reached the Session
- **THEN** `cyber channels log` shows whether it was rejected for signature, rate limit or sender rules

### Requirement: Hold delivery approval
(P3) Events admitted with `delivery: hold` SHALL appear in the TUI, web and mobile clients as pending inbound items that the user can release or drop through the Session inbox routes defined by `server-api` (`POST /api/v1/sessions/:sessionID/inbox/:messageID/release|drop`). Unreleased held events SHALL expire after 24 hours.

#### Scenario: Review before acting
- **WHEN** a `hold` channel receives a production alert
- **THEN** it waits as a pending item until the user releases it into the Session

### Requirement: Channels API
(P3) The server SHALL expose `GET|POST /api/v1/channels`, `GET|PATCH|DELETE /api/v1/channels/{chn_id}`, `POST /api/v1/channels/{chn_id}/event`, `POST /api/v1/channels/{chn_id}/test` and `GET /api/v1/channels/{chn_id}/log`.

#### Scenario: Create via API
- **WHEN** an SDK client posts a webhook Channel definition
- **THEN** the response includes the Channel ID, URL and one-time secret
