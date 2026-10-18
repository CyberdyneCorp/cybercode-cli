## MODIFIED Requirements

### Requirement: Addressing
(P1) Messages SHALL be addressed by Session ID (`ses_…`), unique Session name, subagent ID or name, teammate name, or, from P2 for configured peers ("Direct peer transport") and from P3 through the Relay, `name@machine` / `ses_…@machine` for remote Sessions. A Session's name SHALL be its title, matched exactly and case-sensitively among non-archived Sessions that are not child Sessions; ID prefixes and fuzzy matches SHALL NOT resolve. A leading `@` SHALL be ignored once, so `@migration` addresses `migration`. Names scoped to the caller SHALL take precedence, in this order: team names when the caller is a team member (`lead`, `all`, teammate names), then the caller's subagents, then server-wide Session names. An address matching several Sessions in the tier that resolves it SHALL fail with `AmbiguousAddressError` listing the candidates. An address that resolves to no reachable Session SHALL fail with `AddressNotFoundError`, an address containing `@` that no available remote transport can reach SHALL fail with `AccountRequiredError`, and an address resolving to the sending Session itself SHALL fail with `InvalidRequestError` (`self_target`).

#### Scenario: Ambiguous name
- **WHEN** two local Sessions are named `migration` and a message is sent to `migration`
- **THEN** the send fails with `AmbiguousAddressError` listing both Session IDs

#### Scenario: Subagent name shadows a Session name
- **WHEN** a Session has a subagent named `api` and an unrelated Session on the server is titled `api`
- **THEN** a message from that Session to `api` reaches its subagent, and no ambiguity error is raised

#### Scenario: No prefix matching
- **WHEN** a message is sent to `ses_01J` and no Session has exactly that ID or title
- **THEN** the send fails with `AddressNotFoundError`, even if one Session ID starts with `ses_01J`

### Requirement: Inbound controls
(P1) Each Session SHALL apply `messaging.inbound` with a default policy and optional per-sender rules. The policy SHALL be `deliver`, `hold` or `refuse`. `messaging.inbound` SHALL accept `default` and the pattern lists `deliver`, `hold` and `refuse`, matched with the permission wildcard syntax against each of the sender's addresses (its Session ID, its name, `name@machine` for off-machine senders, `client` or `mcp:<label>` for clients). A `refuse` match SHALL win over a `hold` match, and a `hold` match over a `deliver` match. Without a match, a Session-level override (set at creation or with `PATCH /api/v1/sessions/:sessionID`) SHALL apply when present, then `default`. Defaults SHALL be `deliver` for same-machine senders and `hold` for cross-machine senders, including peers once "Direct peer transport" is available. A `hold` outcome SHALL admit the message with `delivery: "hold"`, so it reaches the model only after the user releases it or a later mode or settings change allows it. A held cross-session message SHALL NOT be editable; it SHALL only be released or dropped.

#### Scenario: Held message released
- **WHEN** a cross-machine message is held and the user approves it in the TUI
- **THEN** it is released into the inbox and delivered at the next Safe Boundary

#### Scenario: Refused sender
- **WHEN** `messaging.inbound.refuse` lists the sender's address pattern
- **THEN** the message is dropped, the sender sees `refused`, and nothing is stored in the target transcript

#### Scenario: Refuse wins over deliver
- **WHEN** `messaging.inbound.deliver` lists `front*` and `messaging.inbound.refuse` lists `frontend`
- **THEN** a message from `frontend` is refused with reason `inbound_rule`

### Requirement: Rate limits
(P1) The system SHALL accept at most 10 messages per minute from one sender to one target, and at most 30 pending undelivered messages per target inbox. Excess messages SHALL be refused with reason `rate_limited` in the sending Session. The per-minute limit SHALL count messages with outcome `delivered` or `held` in a sliding 60-second window kept in durable records, so a restart does not reset it, and refused messages SHALL NOT count. A sender SHALL be its Session ID, `client:<label>` for a Messaging API client, `mcp:<mcp_client>` for MCP serve without a caller Session, or its `name@machine` for an off-machine sender. Pending messages SHALL be the target's unpromoted cross-session messages, held ones included. The refusal SHALL carry `limit: "per_target_minute"` or `limit: "inbox_pending"` and SHALL be recorded only in the sending Session.

#### Scenario: Message storm
- **WHEN** a looping agent sends its 11th message within a minute to the same target
- **THEN** the 11th is refused with `rate_limited`

#### Scenario: Full inbox
- **WHEN** a target holds 30 unreleased held messages and another Session sends it a message
- **THEN** the message is refused with `rate_limited` and `limit: "inbox_pending"`, and the target inbox is unchanged

#### Scenario: Restart keeps the window
- **WHEN** a sender has sent 10 messages to one target and the server restarts within the same minute
- **THEN** an 11th message within that minute is still refused with `rate_limited`

### Requirement: Non-interactive sessions
(P1) A `cyber exec` Session SHALL default to `messaging.inbound: refuse` unless `--accept-messages` is passed. `cyber exec` SHALL apply this by creating its Session with the Session-level inbound override `refuse` and reason `non_interactive`; a Session it reuses SHALL keep its policy, with a warning. Held messages in Sessions with no attached client SHALL remain held until a client attaches or the Session ends, at which point they SHALL be dropped and the senders notified. Deleting a Session SHALL drop its held messages and notify their senders with reason `session_deleted` before the Session's records are removed.

#### Scenario: Exec session receives a message
- **WHEN** a message targets a running `cyber exec` Session started without `--accept-messages`
- **THEN** the message is refused with reason `non_interactive`

#### Scenario: Deleted Session with held messages
- **WHEN** a Session holding two held messages is deleted
- **THEN** both senders record `messaging.message.dropped.1` with reason `session_deleted`

### Requirement: Messaging API
(P1) The server SHALL expose `GET /api/v1/messaging/sessions`, `POST /api/v1/messaging/send`, `GET /api/v1/sessions/:sessionID/messaging` (history) and `POST /api/v1/messaging/watch`, so clients and SDKs can message Sessions without going through a model. Held messages SHALL be released or dropped through the Session inbox routes defined by `server-api` (`POST /api/v1/sessions/:sessionID/inbox/:messageID/release|drop`); messaging SHALL NOT define separate release routes.
- `GET /api/v1/messaging/sessions` SHALL accept `from` (the listing caller's Session) and `directory`, and return the "Reachable session listing" entries.
- `POST /api/v1/messaging/send` SHALL accept `{ id?, from?, to, text, notify_when_idle? }` and answer `200` with `{ message_id, outcome, reason?, limit?, to: { address, session_id? }, watch_id?, replayed? }`; a refused outcome SHALL be a `200` result, not an error. Without `from` the sender SHALL be the client (origin `client`); with `from` the message SHALL be sent as that Session. A repeated `id` SHALL return the original result with `replayed: true`.
- `GET /api/v1/sessions/:sessionID/messaging` SHALL page the Session's sent and received messages (`server-api` pagination, filter `direction`) as `{ message_id, direction, from, to, origin_kind, outcome, reason?, size, sha256, text?, created_at, updated_at }`, with no `text` for refused messages.
- With messaging disabled, the send and watch routes SHALL answer `503` `ServiceUnavailableError` with `service: "messaging"`.

#### Scenario: Human message from the SDK
- **WHEN** a script posts to `/api/v1/messaging/send` targeting a running Session
- **THEN** the message is delivered under the same inbound controls, with origin `client`

#### Scenario: Release a held message
- **WHEN** a client posts to `/api/v1/sessions/ses_1/inbox/msgx_7/release`
- **THEN** the held cross-session message is promoted at the next Safe Boundary and `messaging.message.released.1` is emitted to the sender

#### Scenario: Retry after a timeout
- **WHEN** a client posts the same `id` twice because the first response was lost
- **THEN** the second response carries the first outcome with `replayed: true` and the target inbox holds the message once

### Requirement: Usage attribution
(P1) Tokens spent processing a delivered message SHALL be attributed to the receiving Session, tagged `origin: session`, so cost reports can separate user-driven usage from message-driven usage. Usage of runs started by MCP input (a prompt or message admitted through MCP serve, `mcp-server`) SHALL be tagged `origin: mcp` with `mcp_client`; a message sent through MCP keeps its Messaging API origin for inbound rules. Each provider step SHALL carry the origin of the most recently promoted input of its Session: `mcp` for MCP input, `session` for a message sent by another Session's agent (and, with "Direct peer transport", by a peer Session), `client` for a message sent through the Messaging API or `cyber messages send`, `job` for a background handback, `user` for user input, and otherwise the admission source (`workflow`, `hook`, `loop`, `goal`, `channel`). The origin SHALL be recorded on `session.step.started.1`, on the step's usage records and in the usage ledger; records made before origins existed SHALL read as `user`.

#### Scenario: Cost report
- **WHEN** the user runs `cyber stats --by origin`
- **THEN** message-driven usage is shown separately from user prompts

#### Scenario: MCP-driven usage
- **WHEN** an MCP client labelled `claude-code` prompts a Session and the run spends 3 Turns
- **THEN** the `usage.recorded.1` records of those Turns carry `origin: mcp` and `mcp_client: claude-code`

#### Scenario: Handback usage
- **WHEN** a background subagent's handback starts a Turn in its parent Session
- **THEN** that Turn's usage is tagged `origin: job`, not `session`

#### Scenario: Client message usage
- **WHEN** a script sends a message through `POST /api/v1/messaging/send` without `from` and the receiving Session spends one Turn on it
- **THEN** that Turn's usage is tagged `origin: client`

## ADDED Requirements

### Requirement: Message chain limit
(P1) Each message SHALL carry `hops`: one more than the hops of the sending run, where a run's hops is the largest `hops` among the messages and idle notices promoted into it, and 0 for a run that none joined. A send whose `hops` would exceed `messaging.max_hops` (default 8, range 1–64) SHALL be refused with reason `chain_limit` in the sending Session. Hops SHALL be run-scoped, so a later run started by user input begins at 0.

#### Scenario: Ping-pong between two models
- **WHEN** Sessions A and B each answer every message they receive with a message to the other, starting from a user prompt in A
- **THEN** the ninth message in the chain is refused with `chain_limit`, and neither Session is prompted further by the other

#### Scenario: User restarts the conversation
- **WHEN** a Session whose last run reached the chain limit is prompted by the user
- **THEN** a message sent in the new run carries `hops: 1` and is delivered

### Requirement: Messaging CLI
(P1) `cyber messages send <to> [text | -] [--from <session>] [--notify-when-idle]` SHALL send through the Messaging API, reading the text from stdin with `-`, and SHALL exit 0 for `delivered` or `held`, 1 for `refused` with the reason on stderr, 2 for `AmbiguousAddressError` (listing the candidates), `MessageTooLargeError` and other usage errors, and 6 for `AccountRequiredError`. `cyber messages list [--session <session>] [--direction in|out] [--limit N]` SHALL print a Session's message history (default: the Location's most recent Session) with time, direction, peer address, outcome, size and the first line of the text; `cyber messages list --reachable [--from <session>]` SHALL print the reachable Sessions. Both SHALL support `--format json`.

#### Scenario: Send from a script
- **WHEN** the user runs `echo "deploy is done" | cyber messages send migration -`
- **THEN** the message is delivered to the Session named `migration`, the CLI prints its `msgx_` ID and outcome, and exits 0

#### Scenario: Refused send
- **WHEN** the target refuses messages from clients
- **THEN** `cyber messages send` prints the reason `inbound_rule` to stderr and exits 1
