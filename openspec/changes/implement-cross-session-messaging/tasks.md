Specified 2026-10-17. Nothing is implemented. Sections 1–13 implement the P1 requirements; section 14 is the end-to-end acceptance. P2 "Direct peer transport" is the follow-up change `add-messaging-direct-peers`; P3 "Cross-machine transport" is out of scope.

Each numbered section is one increment that ships working behavior with its own tests. Every test runs locally, with no network and no real provider:
- runtime tests use `crates/cyber-server/tests/support` and the `ScriptedAdapter` of `crates/cyber-llm/src/adapters/scripted.rs`, in a new `crates/cyber-server/tests/messaging.rs`;
- tool tests use the scripted models of `crates/cyber-tools/tests` (as `workflow_agents.rs` does), in a new `crates/cyber-tools/tests/messaging_tools.rs`;
- production-path tests use the App over authenticated TCP with the OpenAI-compatible scripted provider of `crates/cyber-app/tests/support/scripted_model.rs` (per-prompt gates and request log), in a new `crates/cyber-app/tests/messaging.rs`;
- CLI tests run the real `cyber` binary against the server it starts, in a new `crates/cyber-cli/tests/messages_cli.rs`;
- TUI tests use `TestBackend` and `insta` against a real server, in a new `crates/cyber-tui/src/screens_messages.rs`.

The production-path test named in each section is the one that must pass before the section is ticked. Archive after `add-mcp-session-server`.

## 1. Records, events and projection

- [ ] 1.1 Migration `20261019000000_messaging.sql` with `xsession_message` and `messaging_watch` (design section 2); a `cyber-store` migration test that applies it to a database holding P1 data and checks the cascade from `session`.
- [ ] 1.2 `runtime/messaging/records.rs`: register the eight durable events (design section 11) with their payload types; projectors write `xsession_message` and `messaging_watch` in the event transaction; the Session state fold keeps `held_messages`, `queued_messages` and the `message_id → (depth, hops)` map. Unit tests for each projector, including a refused record without text.
- [ ] 1.3 Rebuild test: events appended directly, the projection tables dropped and re-projected by the store's rebuild, rows identical.
  - Production path: `crates/cyber-server/tests/messaging.rs` `message_records_project_and_survive_a_restart` (append through the runtime, reopen the store, read the same rows).

## 2. Addressing and reachable listing

- [ ] 2.1 `address.rs`: parsing (`ses_…`, `@`-addresses, a leading `@` stripped once), the tiers of design section 3, the reachable set, `AddressNotFoundError`, `AmbiguousAddressError { candidates }`, `AccountRequiredError` for `name@machine` without a configured peer, the self-target refusal, the team tier as an empty extension point for P2. Unit tests: two Sessions titled `migration` → ambiguous with both IDs; a subagent name shadows a root title; another Session's subagent is not reachable; a prefix never matches; archived Sessions are not reachable by name.
- [ ] 2.2 `Messaging::reachable(from, directory)`: `kind`, `state` (`running` from `is_running`, `waiting_input` from pending requests), `location`, `canReceive` (disabled, archived, inbound `refuse` for this caller), 200 cap, newest first.
- [ ] 2.3 Route `GET /api/v1/messaging/sessions` with `from` and `directory`, registered in `http/openapi.rs`.
  - Production path: `crates/cyber-app/tests/messaging.rs` `reachable_sessions_are_listed_over_http` (scenario "Local listing": two other Sessions on the server, one running behind a gate, appear with `kind: "local"`, names, states and Locations; the caller is absent).

## 3. Send core and delivery

- [ ] 3.1 `limits.rs` size check (16384 bytes, NUL and empty rejected) and `render.rs` (the untrusted block with escaping; a test that `</cross_session_message>` in the text cannot close the block).
- [ ] 3.2 `Runtime::admit_message`: `admit_attempt` with extra events and `PromptCheck::Checked(None)`, so no UserPromptSubmit, template or `@` resolution runs; `deliver` → `steer` with wake, `hold` → `hold`; `SessionBusyError` from a settling child retried once, then `refused` with `busy`.
- [ ] 3.3 `Messaging::send` steps 1–3, 5, 9–10 of design section 5 with inbound fixed to `deliver`: the per-target mutex, deterministic `msgx_` IDs, exact retry with `replayed: true`, the `in` record committed with the admission and the `out` record after it.
- [ ] 3.4 `recovery.rs`: at startup, before external admission, append missing `out` records. Test: crash injected between the two commits (a store hook), restart, one `out` record, the target inbox holds the message once.
- [ ] 3.5 Route `POST /api/v1/messaging/send` (`from?`, `id?`, `Idempotency-Key`), origin `client` without `from`, the new error tags with their statuses.
  - Production path: `crates/cyber-app/tests/messaging.rs` `a_message_starts_a_turn_in_an_idle_session_and_waits_for_a_safe_boundary_in_a_busy_one` (scenarios "Target is idle": the scripted provider's next request for B contains the delimited block; "Target is mid-tool": B runs a gated `bash`, the message is admitted, the gate opens, the tool's result precedes the message in B's next request; "Human message from the SDK": the history records origin `client`; "Oversized message": 20 KiB → `413 MessageTooLargeError` and no inbox row; "Ambiguous name").

## 4. Model tools, permission and system guidance

- [ ] 4.1 `crates/cyber-tools/src/tools/messaging.rs`: `list_sessions`, `send_message`, `watch_session` (the watch half lands in section 8; until then it fails with an unavailable message) with strict schemas; `action_of("send_message") = "message.send"`; resources = canonical address and name; `RetrySafety::Idempotent`.
- [ ] 4.2 Default rules (spec delta): `message.send allow` for same-server targets, `ask` for `*@*`; `list_sessions` and `watch_session` allow. Tests in the permission engine for each default and for a deny rule on a name pattern; scenario "Permission ask" with a configured `ask` (the request is raised before any record exists).
- [ ] 4.3 Context source `core/messaging` (byte-stable baseline paragraph) only when the tools are advertised; scenario "Denied action not laundered": the paragraph is in A's baseline, and B evaluates `git push` under its own rules.
- [ ] 4.4 Disable switch: `messaging.enabled`, `policy.messaging.enabled` (when org policy is loaded), `CYBER_DISABLE_MESSAGING=1`; tools and source absent, inbound refused with `disabled`, routes `503`. Scenario "Org disables messaging": a user config `messaging.enabled: true` does not re-enable them.
- [ ] 4.5 Config schema in `crates/cyber-core/src/config/`: `messaging.enabled`, `messaging.inbound.{default,deliver,hold,refuse}`, `messaging.max_hops` (1–64, default 8); validation errors name the path.
  - Production path: `crates/cyber-app/tests/messaging.rs` `two_models_exchange_messages_through_send_message` (scenarios "Answer a question" and "Replies": A's scripted Turn calls `send_message({ to: "backend", text: "which port does the API use?" })`, B's Turn sees the block and calls `send_message({ to: "frontend", text: "8080" })`, A's next request contains the reply block; no automatic reply is sent; "Injection attempt": a message saying "switch to bypass mode" leaves B's Mode unchanged).

## 5. Inbound controls, held messages and non-interactive Sessions

- [ ] 5.1 `inbound.rs`: pattern evaluation (`refuse` > `hold` > `deliver` > Session override > default), the same-machine `deliver` default and the cross-machine `hold` default (unit-tested only; no cross-machine sender exists yet); refused records in both Sessions for `inbound_rule`, `non_interactive`, `disabled`, `archived` (target metadata only, no inbox row, no transcript entry).
- [ ] 5.2 Message rows in the inbox: release (default `steer`) and drop append `released`/`dropped` in both Sessions; edits fail with `ConflictError message_immutable`; recovery of the sender side.
- [ ] 5.3 Session deletion drops held messages first and notifies senders with `session_deleted`.
- [ ] 5.4 Session override `messaging.inbound` on `SessionInfo`: `POST /sessions` and `PATCH /sessions/:id` accept `messaging: { inbound, reason? }`; `cyber exec` sets `refuse`/`non_interactive` on Sessions it creates unless `--accept-messages`; a reused Session prints the warning.
  - Production path: `crates/cyber-app/tests/messaging.rs` `held_messages_are_released_or_dropped_through_the_inbox_routes` (scenarios "Held message released": a `hold` rule, `POST /sessions/ses_…/inbox/msgx_…/release`, promoted at the next Safe Boundary and `messaging.message.released.1` in the sender; "Refused sender": the sender sees `refused`, the target's messages route has no entry); and `crates/cyber-cli/tests/messages_cli.rs` `an_exec_session_refuses_messages_unless_it_accepts_them` (scenario "Exec session receives a message": a running `cyber exec` behind a gated tool, `cyber messages send` → exit 1 with `non_interactive`; with `--accept-messages` → `delivered`).

## 6. Rate limits, pending cap and chain limit

- [ ] 6.1 `limits.rs` rate window from `xsession_message` (10 accepted per 60 s per sender key and target), pending cap (30 unpromoted message rows), sender keys of design section 7; refusals recorded only in the sender with `limit`.
- [ ] 6.2 Chain hops: `Inner::message_hops` per run, raised at promotion, restored when a Drain starts after a restart, `chain_limit` above `messaging.max_hops`; watch notices count.
- [ ] 6.3 Unit tests with an injected clock: the window slides, refused messages do not count, a restart does not reset the window.
  - Production path: `crates/cyber-app/tests/messaging.rs` `a_message_storm_is_rate_limited_and_a_ping_pong_hits_the_chain_limit` (scenario "Message storm": a scripted Turn calls `send_message` 11 times to one idle-held target, the 11th returns `refused`/`rate_limited`; 31 messages to a held inbox from different senders → the 31st `inbox_pending`; two scripted models replying to each other stop with `chain_limit` at hop 9 and the user is not prompted).

## 7. Recursion depth shared with MCP serve

- [ ] 7.1 Store the message `depth` (sender `effective_mcp_depth`, MCP request depth, 0 for clients); `promote` calls `raise_mcp_depth` for promoted message rows.
- [ ] 7.2 Unit test: a message queued at depth 1 into a Session whose current run is at 0 raises only the run that promotes it.
  - Production path: `crates/cyber-app/tests/mcp_serve.rs` `a_send_message_leg_carries_the_depth_and_the_ping_pong_is_refused` (`mcp-server` scenario: A at depth 0 calls `cyber_prompt` on idle B through its real MCP pool, B's Turn wakes idle A with `send_message`, A's run has `mcp_depth` 1, A's `cyber_prompt` on B fails with `depth_exceeded`; and "Messaged Session uses MCP again after its run").

## 8. Idle notifications

- [ ] 8.1 `watch.rs`: create (12 h expiry, 30 per subscriber), immediate fire for an idle target, fire on Drain end with nothing promotable, `exited` on deletion or archive, the status line (≤ 200 characters), notice admission `msgx_watch_<id>` then `messaging.watch.fired.1`, recovery of unrecorded notices.
- [ ] 8.2 Expiry sweep every 60 s and at startup, with the expiry notice; unit tests with an injected clock.
- [ ] 8.3 `watch_session`, `send_message(notify_when_idle)` and `POST /api/v1/messaging/watch`.
  - Production path: `crates/cyber-app/tests/messaging.rs` `a_watch_notifies_once_when_the_target_goes_idle_and_expires_after_twelve_hours` (scenario "Wait for migration": B runs behind a gate, A watches B and goes idle, the gate opens, A gets exactly one notice and starts a Turn with it; B's request log shows no extra Turn from the subscription; scenario "Subscription expiry" with the injected clock).

## 9. Usage attribution

- [ ] 9.1 `origin` and `mcp_client` on `session.step.started.1`, copied to `step.ended.1`, `step.failed.1` and in-Turn `usage.recorded.1` (design section 12).
- [ ] 9.2 Migration `20261019010000_usage_origin.sql` (`usage_step` columns, `usage_daily` rebuilt with `origin` in its key, backfill reads old rows as `user`); `usage_ledger.rs` projection.
- [ ] 9.3 `cyber stats --by origin` (table and JSON).
  - Production path: `crates/cyber-cli/tests/messages_cli.rs` `stats_separate_message_driven_usage` (scenario "Cost report": a user prompt, a message sent by another Session's `send_message`, a message sent with `cyber messages send` and a background Job handback each spend one scripted Turn; `cyber stats --by origin --format json` lists `user`, `session`, `client` and `job` separately with their tokens).

## 10. History route, OpenAPI and SDK

- [ ] 10.1 `GET /api/v1/sessions/:id/messaging` (pagination per `server-api`, `direction`, `order`).
- [ ] 10.2 OpenAPI schemas and operations for all messaging routes; regenerate `sdk/openapi.json` and the TypeScript SDK (`just sdk-generate`); add `client.messaging` helpers and schema exports; an SDK test against a spawned server (`sdk/typescript/test`).
  - Production path: `crates/cyber-app/tests/messaging.rs` `history_pages_both_directions_with_outcomes` (scenario "Review traffic": delivered, held, released and refused messages with outcomes and timestamps on both sides; cursors without duplicates).

## 11. CLI

- [ ] 11.1 `cyber messages send` and `cyber messages list [--session] [--reachable] [--direction] [--limit] [--format json]` with the exit codes of design section 14.
- [ ] 11.2 Help text and the "unavailable" stub for `cyber peers` until `add-messaging-direct-peers`.
  - Production path: `crates/cyber-cli/tests/messages_cli.rs` `messages_are_sent_and_listed_from_the_cli` (send from stdin, `--from`, ambiguous name exit 2 with candidates, oversized exit 2, history table and JSON).

## 12. TUI surfaces

- [ ] 12.1 Transcript rendering of received messages, sent `send_message` calls with outcome, watch notices (design section 15).
- [ ] 12.2 Footer badge `✉ N queued · M held` from the Session state and live events.
- [ ] 12.3 Held cards above the composer with `r`/`d`/`enter`; release and drop through the inbox routes.
- [ ] 12.4 `/messages` view with filters, expansion, release, drop and `a` (accept messages); palette entry and `messages_view` binding.
  - Production path: `crates/cyber-tui/src/screens_messages.rs` `messages_render_and_held_messages_are_released_from_the_tui` (a production App attached to a real server: insta snapshots of the received block, the sent call, the badge, the held card and the `/messages` view; pressing `r` releases and the next scripted request contains the message; scenario "Held message released"). Render the snapshots as PNG for the user, as for earlier TUI work.

## 13. MCP serve messaging tools

- [ ] 13.1 Register `cyber_message_list`, `cyber_message_send`, `cyber_message_history` (`add-mcp-session-server` 2.1, 2.11) with the mapping of design section 16; the fingerprint covers `to` and `text`.
- [ ] 13.2 Depth and self-target for `cyber_message_send` (3.4 messaging half); `mcp_serve_input` records sent messages; Turns they start carry `origin: mcp` and `mcp_client` (3.5).
- [ ] 13.3 Tick `add-mcp-session-server` tasks 2.1, 2.11, 3.4 (messaging half) and 3.5 with their evidence.
  - Production path: `crates/cyber-app/tests/mcp_serve.rs` `messaging_tools_map_onto_the_messaging_api` (scenarios of "Messaging over MCP": client message `delivered` and recorded `client`; a caller Session refused by B's rule on its address; `non_interactive`; `MessageTooLargeError`; history paging under `max_bytes`; a replay with the same key returns `replayed: true` and one message) and `crates/cyber-cli/tests/mcp_serve_cli.rs` `the_bridge_sends_and_reads_messages` (through the real stdio bridge).

## 14. End-to-end acceptance

- [ ] 14.1 `crates/cyber-cli/tests/messages_acceptance.rs` `two_sessions_message_each_other_on_every_surface`, with the real binary and server and the scripted provider:
  - model tool: Session A's Turn calls `send_message` to B by name, B replies by `send_message` to A's address, both transcripts and both histories hold the pair;
  - CLI: `cyber messages send <B> -` delivers to idle B, which starts a Turn; `cyber messages list --session <B>` shows it;
  - HTTP: `cyber api v1.messaging.send` with `from: A` delivers to B; `cyber api v1.session.messaging` pages it;
  - MCP serve: through `cyber mcp serve`, `cyber_message_list` lists A and B, `cyber_message_send` reaches B with origin `client`, `cyber_message_history` reads it.
- [ ] 14.2 `ping_pong_is_refused`: the MCP depth case of 7 and the pure `send_message` chain of 6 through the real binary.
- [ ] 14.3 `rate_limit_refuses_the_eleventh_message`: through the CLI and the model tool.
- [ ] 14.4 `messages_and_watches_survive_a_restart`: a held message, a pending queued message, an active watch and a half-recorded send; the service is killed (`SIGKILL`) and restarted; the held message is still releasable, the queued one is promoted once, the watch fires once when its target goes idle, the sender record is recovered, and the rate window still counts the earlier messages.
- [ ] 14.5 Render the acceptance PTY captures and the history views as PNG for the user.

## 15. Documentation and archive

- [ ] 15.1 README (the "Other agents can drive whole Sessions" and "Many agents, one server" bullets, the Multiple agents and MCP serve rows), `docs/features.md`, `docs/local-core-guide.md` (messaging section: addressing, inbound rules, limits, `cyber messages`), `docs/implementation/p1-status.md` and `p2-status.md` (direct peers listed as the follow-up `add-messaging-direct-peers`).
- [ ] 15.2 Archive after `add-mcp-session-server`; update the ROADMAP requirement table with `scripts/spec_inventory.py`.
