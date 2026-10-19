## Why

Cross-session messaging is the next roadmap step after the P1 acceptance audit (ROADMAP P1 "`cross-session-messaging` (same machine)", P2 "(direct peers)"). Nothing of it exists in code: no `send_message`, `list_sessions` or `watch_session` tool, no `/api/v1/messaging` route, no `xsession_message` table, and no `cyber messages` command (`crates/cyber-core/src/config/validate.rs:37` only accepts the `messaging` config key). Two pieces of work wait on it:

- **`cyber mcp serve`** ships 7 of its 10 tools. Tasks 2.1, 2.11, 3.4 (messaging half) and 3.5 of `add-mcp-session-server` are deferred because there is no Messaging API to map `cyber_message_list`, `cyber_message_send` and `cyber_message_history` onto, no message depth to carry the recursion guard across `send_message`, and no usage `origin` on step records.
- **Agent teams** (`agent-teams`, P2, next on the roadmap) message teammates "using the local transport of cross-session-messaging", including names (`lead`, `all`, teammate names), idle notices to the lead and durable message history across `/team resume`.

The runtime already has what delivery needs: the durable inbox (`session_input`, `session.prompt.admitted.1`, `steer`/`queue`/`hold` delivery, Safe Boundary promotion in `runtime/drain.rs`), the release and drop routes `POST /api/v1/sessions/:id/inbox/:message_id/release|drop`, the idempotent handback pattern of Jobs (`runtime/jobs.rs` `notify_job`, `msg_handback_<job>`, `job.notified.1`, recovery) and Workflow Runs (`runtime/workflow_results.rs`, `msg_workflow_<run>`, `workflow.run.notified.1`), and the run-scoped `mcp_depth` (`runtime/mcp_depth.rs`).

## What Changes

In scope: every P1 requirement of `cross-session-messaging` (local transport on one server). Out of scope:
- the P2 requirement "Direct peer transport" (`peers` config, `cyber peers`, peer listing, sends, outcome polling, `cyber attach <session>@<peer>`, mDNS discovery). It moves to a follow-up change, `add-messaging-direct-peers`, to be specified after this one lands; the design leaves its extension points (the `@` address tier, the off-machine `hold` default, the sender kind) in place.
- the P3 requirement "Cross-machine transport" (Relay, Cyber Account, `cyber:messaging` scope, org members), the remote parts of "Reachable session listing" and "Outcome reporting", and the account audit log of "Transcript and audit".

Until peers exist, any address containing `@` fails with `AccountRequiredError`, as the P3 scenario "Not logged in" states.

- **Runtime (`runtime/messaging/`).** Address resolution (Session ID, unique title, subagent name, the team tier as an extension point for `agent-teams`), the reachable listing, a send pipeline with ordered checks (size, address, self, disable switch, inbound rules, rate limits, chain hops), admission of the message into the target inbox as an untrusted delimited block with `source: "session"` and the `msgx_` ID, durable records in both Sessions, crash recovery modeled on the Job handback, idle watches with 12-hour expiry, and drops of held messages when a Session is deleted.
- **Model tools.** `list_sessions`, `send_message({ to, text, notify_when_idle? })` and `watch_session({ target })`, gated by `message.send` (resource: the canonical target address), with fixed system-prompt guidance against laundering denied actions.
- **Inbound controls.** `messaging.inbound` (`default`, plus `deliver`/`hold`/`refuse` sender patterns), the same-machine `deliver` default (cross-machine `hold` stays for later transports), a Session-level override that `cyber exec` sets to `refuse` (`non_interactive`) unless `--accept-messages` is passed.
- **Limits.** 16 KiB per message, 10 messages per minute per sender and target, 30 pending messages per target inbox, all refused with `rate_limited`; a new chain limit (`messaging.max_hops`, default 8) refuses message ping-pong with `chain_limit`.
- **Recursion.** Messages carry the sender's `mcp_depth`; promotion raises the receiving run's depth, so the `mcp-server` guard stops loops where one leg uses `send_message`.
- **Usage attribution.** Every provider step records `origin`: `session` for messages from agents, `client` for messages sent through the API or CLI, `job` for background handbacks, `mcp` for MCP input, `user` for user prompts, and the admission source otherwise (`workflow`, `hook`, ...); recorded with `mcp_client` on `session.step.started.1`, `usage.recorded.1` and the `usage_step` ledger; `cyber stats --by origin`.
- **Events.** Durable `messaging.message.delivered|held|refused|released|dropped.1` in both Sessions, `messaging.watch.created|fired|expired.1` in the subscriber.
- **HTTP.** `GET /api/v1/messaging/sessions`, `POST /api/v1/messaging/send`, `POST /api/v1/messaging/watch` and `GET /api/v1/sessions/:id/messaging`; new tagged errors; OpenAPI and regenerated TypeScript SDK.
- **CLI.** `cyber messages list|send`, `cyber exec --accept-messages` and `cyber stats --by origin`.
- **TUI.** Received and sent message rendering, an inbox indicator in the footer, held-message cards with release and drop, and a `/messages` view.
- **MCP serve.** `cyber_message_list|send|history` mapped onto the Messaging API, completing `add-mcp-session-server` tasks 2.1, 2.11, 3.4 and 3.5.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `cross-session-messaging`: "Addressing" (scoped name precedence, no ID prefixes, `@` handling), "Inbound controls" (config shape and precedence, Session override), "Rate limits" (sender keys, window, what counts), "Non-interactive sessions" (how exec marks its Session), "Messaging API" (body, results, history paging), "Usage attribution" (origin values; restates the `add-mcp-session-server` text); added "Message chain limit" and "Messaging CLI".
- `permissions-modes`: "Default rules" (`message.send allow` for same-server targets; `list_sessions` and `watch_session` allow).
- `server-api`: "Tagged error model" (messaging errors and statuses).
- `tui`: added "Cross-session message surfaces".
- `cli-commands`: "Stats command" (`--by origin`).
- `observability-costs`: "Usage commands" (`--by origin`).
- `system-context`: "Typed Context Sources" (the built-in `core/messaging` guidance source).

## Impact

- Code: `crates/cyber-store/migrations` (new tables, usage origin columns), `crates/cyber-server/src/runtime/messaging/` (new), `runtime/drain.rs` (promotion-time depth and hops), `runtime/mod.rs` (admission with extra events, inbox actions on message rows, deletion), `runtime/usage_ledger.rs`, `crates/cyber-server/src/http/messaging.rs` (new) and `http/openapi.rs`, `http/mcp_serve/` (3 tools), `crates/cyber-tools/src/tools/messaging.rs` (new) and permission defaults, `crates/cyber-core/src/config/` (`messaging`), `crates/cyber-cli/src/commands/messages.rs` (new), `exec`, `stats.rs`, `crates/cyber-tui` (rendering, footer, `/messages`), `sdk/openapi.json` and `sdk/typescript`.
- Docs: README feature rows (messaging and MCP serve "7 of 10" lines), `docs/features.md`, `docs/implementation/p1-status.md` and `p2-status.md`.
- Sequencing: archive after `add-mcp-session-server`, because both modify `cross-session-messaging` "Usage attribution" and this change restates that change's text.
- Canonical inventory after archive: +3 P1 requirements (`cross-session-messaging` "Messaging CLI" and "Message chain limit", `tui` "Cross-session message surfaces"); 6 requirements of `cross-session-messaging` and 5 of other capabilities modified, with no phase changes.
