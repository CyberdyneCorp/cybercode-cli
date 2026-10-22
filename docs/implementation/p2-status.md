# P2 implementation status

P2 evidence lives here and is kept separate from the P1 files (`p1-status.md`, `p1-handoff.md` and `p1-acceptance-audit.md`). The plan, the requirement-to-task mapping and the scope exclusions are in [`openspec/changes/implement-m2-1-workflow-runtime`](../../openspec/changes/implement-m2-1-workflow-runtime/proposal.md).

## Goals, slice 2 (`implement-m2-3-goals`, 2026-10-22)

- Templates: `/goal use <name> [text]` and `POST .../goals {template, args}` (`GoalTemplateNotFoundError` 404). Test: `goal_use_template_substitutes_arguments` (`crates/cyber-app/tests/goals.rs`).
- Fork: `/goal fork` and `POST .../goals/fork` start a root Session in a `goal-<ulid>` worktree on `cyber/goal-<ulid>`. Root worktree Sessions record `source_directory`, so `cyber sessions list` in the source checkout lists forks and worktree teammates. Test: `goal_fork_starts_background_session_in_worktree` (regression: listed from the source checkout).
- Goals on agents: the `agent` tool and workflow `agent()` accept `goal`; results carry `goal_status`; a workflow agent's goal budget is capped by the run's remaining budget. Tests: `agent_tool_goal_returns_goal_status`, `workflow_agent_with_goal_runs_until_check_passes` (`crates/cyber-app/tests/workflows.rs`), `an_agent_goal_budget_is_capped_by_what_the_run_has_left`.
- The model `goal` tool behind `goal.set` (ask by default). Test: `the_model_goal_tool_asks_by_default`, golden `crates/cyber-tools/tests/goldens/goal.txt`.
- Unattended asks pause the Goal with `Waiting for approval` after `goals.unattended_ask_seconds` without a watching client. Test: `unattended_ask_pauses_goal_waiting_for_approval` (`crates/cyber-server/tests/goals.rs`).
- Hooks and restart: `goal_completed_hook_receives_goal_json`, `restart_mid_goal_evaluates_before_continuing` (a rebuilt App).
- Workflow runs defer evaluation (M2.1 5.8): `goal_evaluation_waits_for_the_sessions_active_run`.
- `cyber exec --goal` SIGINT exits 130: `exec_goal_sigint_exits_130`.
- TUI: panel keys, sidebar Goal section; snapshots `goal_panel_120x40`, `goal_sidebar_160x40`, `goal_sidebar_light_160x40`.
- Not done: the live end-to-end acceptance on the real binary (task 10.2) and archiving (10.3).

## Agent teams (2026-10-20)

[`implement-agent-teams`](../../openspec/changes/implement-agent-teams/proposal.md) (spec: `agent-teams`, P2; ROADMAP M2.5, teams part) is implemented in sections 1 to 8, each with its production-path test (the App over authenticated TCP in `crates/cyber-app/tests/teams.rs`, the real binary in `crates/cyber-cli/tests/teams_cli.rs`, the TUI against a real server in `crates/cyber-tui/src/screens_team.rs`, plus `crates/cyber-server/tests/teams.rs` and `sdk/typescript/test/teams.test.ts`). Everything is behind the experimental `teams` feature, off by default. It builds on same-server messaging (the resolver's team tier, `Messaging::send`, watches and `xsession_message`).

### End-to-end acceptance (task 9.1, by hand, macOS)

The real `target/debug/cyber`, a real `cyber serve`, the TUI in a pty captured with pyte (HTML, PNG and text), and a local-only scripted OpenAI-compatible provider with per-Session scripted Turns, gates and one-shot usage. No network and no live model. Every `cyber serve` and provider process was stopped by PID. The driver and captures were kept in the session scratchpad and are not in the repository or CI. 15 scenarios, all pass:

| Scenario | Observed | Result |
| --- | --- | --- |
| Feature off by default, then enabled | `cyber features list` shows `teams experimental off (default)`; `cyber features enable teams` writes `cyber.jsonc`; then `teams experimental on (global:…)` | pass |
| Definition from `.cyber/teams/feature.md` (trust-gated) | held until `cyber trust approve --digest sha256:…`; then `cyber teams definitions` lists `feature project api,ui,tests Ship the orders feature across API, UI and tests` | pass |
| `/team start feature` in the TUI | toast `Team feature started: api, ui, tests` and the team view; `api` and `ui` in worktrees on `cyber/team-<ulid>/api` and `/ui`; `tests` shared, owning `tests/**` | pass |
| Tasks with a dependency | `ui` starting `Orders UI` (depends on `Orders API`) gets `TaskDependencyError: Task depends on unfinished task_…` | pass |
| Concurrent claim | `api` and `ui` claim `Order fixtures` with both model replies held at a gate and released together: `ui` wins, `api` gets `TaskClaimedError: Task already claimed by ui` | pass |
| Team view and board | members table (lead, api, ui, tests with status, task, cost) over `todo(2)`/`in_progress(1)`/`blocked`/`done`; the `Orders UI` card shows `@ui · waits 1` | pass |
| Teammate question and reply | `ui` → `lead` `which API shape should I use?` shows in the lead's transcript as an untrusted card and is in the lead's next request; the lead's reply is in `ui`'s next request | pass |
| Labelled teammate request | `tests` running `docker compose up` shows `[tests] Allow bash: docker compose up?` in the lead's TUI with the member's waiting dot; rejected there, `tests` gets `Rejected by user`; `api`/`ui` sandbox escalations for `git commit` approved from the lead as `[api]`/`[ui]` | pass |
| Idle notice | the lead receives `tests is idle with task_… in progress: idle`, then `tests is idle: all assigned tasks done`, drawn as `◷ …` notice lines | pass |
| Ownership denial | the lead editing `tests/orders_test.py` (owned by `tests`) gets `Path owned by teammate tests`; the file is unchanged | pass |
| `team_merge` clean | `team_merge member=api` → `Merged api as cc527b4c…`; the lead's `README.md` says `Orders: API v1` | pass |
| `team_merge` with a conflict | `team_merge member=ui` → `Merge of ui stopped: conflicts in README.md. Your checkout is unchanged.`; the lead's HEAD and `git status` byte-identical before and after | pass |
| `/team resume` after a server restart | `/team stop`, SIGTERM the server, start a new one, `cyber --resume <lead>`, `/team resume`: stopped → active, board identical, `api`'s next request carries its prior history | pass |
| Budget stop | `max_cost_usd 2.0`, one `api` Turn priced at $2.10: `ui`'s next Turn does not start, the team stops (`budget`), and the lead gets `Team budget exceeded (max_cost_usd 2.00, spent 2.12): lead $0.01, api $2.10, ui $0.01, tests $0.00` | pass |
| `cyber teams show` / `list` | `feature stopped (budget)`, spend and budget, members with status, current task, cost and last message, and the board by status | pass |

Bugs found and fixed, each with a regression test:
- TUI: teammate idle notices (`msgx_team_idle_…`) were drawn in the lead's transcript as a user prompt showing the raw `<team_idle_notice …/>` element. `Notice::of` (`crates/cyber-tui/src/messaging.rs`) now recognises them and `crates/cyber-tui/src/view/messaging.rs` draws ` ◷ tests is idle: all assigned tasks done`. Tests: `messaging::tests::team_idle_notices_carry_their_unescaped_line` and `screens_team::a_teammates_idle_notice_is_drawn_as_a_notice_in_the_leads_transcript` (real server, real `view::draw`).
- CLI: `cyber teams list` and `show` used fixed column widths; 31-character team IDs, `stopped (budget)` and long task cells shifted every later column. `crates/cyber-cli/src/commands/teams.rs` now sizes each column to its widest cell. Assertions added to `crates/cyber-cli/tests/teams_cli.rs`; only the `list` check fails on the old code (the `show` test's task cell fits the old 40-character column).

Product notes, not changed:
- Under the default workspace sandbox no Session can write `.git`, so a teammate commits only through a sandbox escalation, which is routed to the lead as a labelled request (fails closed). `team_merge` in the acceptance run depended on those approved commits.
- Resuming a team still over its budget stops it again at the next teammate Turn, as designed; the resume scenario therefore followed a user stop, and the budget stop ran last.
- When a teammate's Turn ends on a rejected permission, its idle line reads `<name> is idle with task_… in progress: idle` because there is no assistant text to quote.

### Not done

- An automated end-to-end suite for the scenarios above (the driver is a hand-run script).
- Runs on Linux and Windows (only macOS was run; Windows cfg code is type-checked with clippy for `x86_64-pc-windows-gnu`).
- The `cyber agents` dashboard (the agent-view half of M2.5), distributed teammates (P3), teammate panes (P4), and the archive of `implement-agent-teams`.

## Cross-session messaging: direct peers (2026-10-19)

Same-server messaging (the 18 P1 requirements of `cross-session-messaging`) is implemented by `implement-cross-session-messaging`; its evidence is in the [P1 status](p1-status.md#cross-session-messaging-2026-10-19). The P2 requirement "Direct peer transport" is implemented by [`add-messaging-direct-peers`](../../openspec/changes/add-messaging-direct-peers/proposal.md) (sections 1 to 10 and the review fixes of section 12; the section 11 acceptance is partial, see the acceptance paragraph below; guide: [direct peers](../peers.md)). Before it, the P1 code kept its extension points: an address with `@` (`name@machine`) was parsed into its own tier and failed with `AccountRequiredError` on every surface (model tool, CLI exit 6, HTTP, MCP serve) with nothing sent (it now resolves configured peers, and `AccountRequiredError` is left for the Relay); the inbound default for a cross-machine sender is `hold` (unit-tested only, since no such sender exists); `message.send` is `ask` for `*@*` targets; the record's `origin_kind` is an open set; `cyber peers` was a stub that exited 2 until section 4 below. The team address tier is an empty extension point for agent teams (M2.5).

Progress on `add-messaging-direct-peers` (2026-10-19): sections 1 (config and errors) and 2 (TLS identity and the peer listener) are implemented. The `peers` key (`listen { address, tls }`, `mdns`, `machine`, `share_paths`, `remotes[] { name, url, fingerprint, enabled, token? }`) is validated and read only from user config: project, local, environment, profile and `--config` layers that set it fail with `ConfigInvalidError` naming the layer and `cyber peers add`; `tls: false` off loopback, plain `http` remotes off loopback, duplicate remote names and the old array shape fail with messages. The tagged errors `PeerUnreachableError`, `PeerAuthError` (`502`), `PeerNotConfiguredError` (`404`) and `PeerScopeError` (`403`) are in the error model, the OpenAPI document (`PeerErrorBody`) and the SDK guards, and `cyp_` peer tokens match the credential redaction patterns. Production-path tests: `cyber-app/tests/peers.rs` (App over HTTP), `cyber-cli/tests/peers_cli.rs` and `git_commit_msg.rs` `commit_msg_redacts_peer_grant_tokens` (real binary). Section 2 (TLS identity and the peer listener) is implemented: with `peers.listen` in user config the server opens a second listener that serves only `/api/v1/peer/*` (every other path `404`, even with the local password), refuses any request with an `Origin` header (`403`), and answers every peer route `401` until grants exist (section 3). Off loopback it serves TLS 1.3 with a self-signed ECDSA P-256 certificate (`rcgen`) whose key lives in `<state>/peer-tls/` (directory 0700, files 0600); the certificate is reissued from the kept key on every start (SANs: machine, `<machine>.local`, `localhost`, the bound IP), so the fingerprint (SHA-256 of the SubjectPublicKeyInfo) changes only with `PeerIdentity::rotate` (`cyber peers invite --rotate-key`, wired with `invite` in section 4). `server.json` records `peer_url` and `peer_fingerprint`; the listener starts and stops with the server. A plain listener opens only on loopback; anything that keeps the listener from opening safely (invalid user config, `tls: false` off loopback, an unreadable key, a port in use) leaves it closed with a logged reason while the main API starts. Production-path tests: `cyber-app/tests/peers.rs` `the_peer_listener_serves_only_peer_routes`, `a_plain_peer_listener_opens_only_on_loopback` and `no_peer_listener_without_listen` (the registered production server through `run_server_until`, over TLS). Section 3 (grants and the peer guard) is implemented: the `peer_grant` table (migration `20261020000000_peer_grant`) and `runtime/peers/grants.rs` create grants with a 256-bit random secret (token `cyp_pgr<ulid>_<43 base64url chars>`, default scope `message`), store only the SHA-256 of the secret, verify in constant time on the reader connection (only a valid token writes, to record `last_used_ms` at most once a minute, re-checking the grant in that write), and refuse revoked or expired grants from the next request on; an active name cannot be reused until revoked. Every `/api/v1/peer/*` route, known or not, needs `Authorization: Bearer cyp_…` of an active grant: no token, the local password, a malformed, unknown, revoked or expired token all get `401` with no detail; routes check their scope (`403 PeerScopeError`, so a `message` grant cannot reach `/api/v1/peer/attach/*`, which is otherwise not served until section 9); each grant may make 120 requests a minute (`429 RateLimitedError` with `retryAfterMs`). The main API refuses `cyp_` tokens (header or `auth_token`) on every transport, with or without a password. `GET /api/v1/peer/hello` answers `{ machine, version, fingerprint, scopes, grant }`. Production-path tests: `cyber-server/tests/peers.rs` `grants_authenticate_scope_and_revoke` (the peer router over TLS through `serve_peer` and the main router through `serve_tcp`) and `cyber-app/tests/peers.rs` `grants_authenticate_on_the_production_peer_listener` (the registered production server). Section 4 (peer client and `cyber peers`) is implemented: `runtime/messaging/peer.rs` calls other servers' peer listeners with a `rustls` verifier that accepts exactly the pinned SPKI hash (no system roots, host names or validity dates; the TLS 1.3 signature is still checked against that key), never follows redirects or uses a proxy, times out connecting after 5 s and a request after 10 s, and maps failures to `PeerUnreachableError` and `PeerAuthError` (`fingerprint_mismatch` before any request byte is sent, `unauthorized` for `401`, `scope` for `403 PeerScopeError`). Tokens are kept by `cyber-app` `peer_tokens` in the OS keyring (service `cyber-code`, account `peer:<name>`), else in the owner-only `<data>/peers-secrets.json` with the warning `peer tokens stored in file (no OS keyring)` (hidden from every sandboxed launch; Windows fails instead of writing a file), and a remote's `token` in user config (`{env:NAME}`) wins. `cyber peers` replaces the stub: `invite <name> [--scope]… [--expires] [--rotate-key] [--host]` creates a grant through the local server (`POST /api/v1/peers/grants`, OpenAPI group `peers`, SDK regenerated) and prints the pairing code `cyberpeer://<host>:<port>/?machine=…&fp=…&t=cyp_…` once; `--rotate-key` makes a new key and swaps the running listener's certificate in place (and `server.json`); `add <name> '<code>' [--token-env NAME]` tests the code before storing anything, stores the token, writes `peers.remotes` and prints the reverse-pairing command; `rm`, `revoke`, `test` (machine, version, scopes, latency; exit 1 with the tag) and `list [--json]` (remote state and latency, grants, `can receive, cannot reply` with the command to run on the other machine). Production-path test: `cyber-cli/tests/peers_cli.rs` `two_binaries_pair_and_test` (two real `cyber` binaries with separate homes, the listener on `127.0.0.1` with TLS); `cyber-server/tests/peers.rs` `the_peer_client_accepts_only_the_pinned_key` covers a server presenting another key. Section 5 (sends between servers) is implemented: an address `X@P` resolves `P` against the enabled `peers.remotes` of user config (read on every send, token from the keyring or `{env:NAME}`) and fails with `PeerNotConfiguredError { peer }` (`404`, CLI exit 2 with the `cyber peers add` hint) before any network request; `AccountRequiredError` is kept for the Relay only. The size, disable switch, chain hops and `message.send` (`ask` for `*@*` by default) run on the sender, which needs a sending Session; the peer client then posts `POST /api/v1/peer/messages` (`{ id, to, text, depth, hops, from_session, from_name? }`, scope `message`). The receiver names the sender after the grant (`Sender::Peer`: `ses_…@<grant>` and `<title>@<grant>`, both trusted only inside that grant's namespace; a body naming another machine is `400`), resolves `X` with the normal tiers minus the caller-scoped ones (never as a further peer), and runs the local pipeline: inbound default `hold` for cross-machine senders, limits counting every Session of the grant together, hops and depth from the body, the untrusted block saying the sender is on another machine, and an `in` record with `origin_kind: "peer"`. `POST /api/v1/peer/messages` is idempotent on the grant and ID, and `GET /api/v1/peer/messages/:id` answers the current outcome of a message the grant sent (`delivered|held|released|dropped|refused`). The sender's `out` record has `origin_kind: "peer"`, `remote_peer` and `remote_session_id` (migration `20261020010000_peer_messages`, also in the history API); a network, TLS or guard failure records outcome `failed` (reason `unreachable`, `fingerprint_mismatch`, `unauthorized` or `scope`, no text) and fails the send with `PeerUnreachableError` or `PeerAuthError`, nothing queued; a retry with the same ID replaces a `failed` record, and an answered one replays locally. `notify_when_idle` to a peer Session is refused, and MCP serve sends only to local Sessions. Production-path test: `cyber-app/tests/peers.rs` `a_message_to_a_peer_is_held_there_and_named_by_grant` (two registered production servers on this host, the model's `send_message` over the TLS peer listener; scenarios "Two laptops on one network", "Sender cannot claim another machine", "Unknown peer name", "Retry after a timeout"). Section 6 (outcome tracking) is implemented: the sender's `out` record of a message a peer answered `held` carries `next_poll_ms` (migration `20261020020000_peer_outcome_poll`, set by the projection 5 s after the send and cleared when the outcome changes), and the server's outcome tracker (`runtime/messaging/tracker.rs`, every 5 s) polls `GET /api/v1/peer/messages/:id` for due rows with the backoff 5 s, 15 s, 1 min, then every 5 min. A release or drop on the peer appends `messaging.message.released.1` or `messaging.message.dropped.1` to the sender Session and publishes the live `messaging.outcome` event (`{ session_id, message_id, to, outcome, reason? }`) on its streams; a message still held after 7 days gets the new `messaging.message.expired.1` (outcome `expired`) and is no longer polled. An unreachable peer, a refused grant or an unknown message leaves the message held and polled again; nothing is resent. Polling resumes after a restart from the stored times. Production-path test: `cyber-app/tests/peers.rs` `a_peer_release_reaches_the_sender_across_a_restart` (two registered production servers, the sender on a database file restarted between the release and the poll, the backoff on a test clock, then the server's own tracker on the real clock; scenario "Release seen by the sender"). Section 7 (listing and watches) is implemented: `GET /api/v1/peer/sessions?limit=` (scope `message`) answers the root Sessions the grant could message (inbound decision for `*@<grant>` not `refuse`), newest first, at most 50, located by workspace name only unless `peers.share_paths`, and `GET /api/v1/peer/sessions/:id/status` answers `{ session_id, state, last_idle_ms }` for one of them (`404` for any other). `list_sessions`, `GET /messaging/sessions` and MCP `cyber_message_list` add, without a `directory` filter, the entries of every enabled remote (asked in parallel, 5 s timeout, cached 15 s per remote) as `X@<peer>` entries of kind `peer`; a remote that does not answer, or has no token, is one `{ address: "*@<peer>", state: "offline", canReceive: false }` entry (`ReachableState` gains `offline`; OpenAPI and SDK regenerated). `watch_session` on `X@P` asks the peer once (unreachable fails the watch, a hidden Session is `AddressNotFoundError`), stores a `messaging_watch` row with `target_peer` and `target_id` `<ses_>@P` (migration `20261020030000_peer_watch`; local idle checks skip these rows), fires at once when the target is idle, and otherwise the server's remote watch poller (every 15 s) fires it when the peer says idle (`exited` when it no longer shows the Session). Unreachable peers leave the watch active; the 12-hour expiry applies as for local watches. `notify_when_idle` to a peer Session is still refused. Production-path tests: `cyber-app/tests/peers.rs` `peer_sessions_are_listed_and_watched` (two registered production servers, the model's `list_sessions` and `watch_session`, the server's own poller; scenarios "Peer offline", "Refused grant hides Sessions", "Watch a remote Session") and `cyber-server/tests/peers.rs` `peer_listings_share_paths_only_when_configured`. Section 8 (discovery) is implemented: with `peers.mdns: true` in user config or `cyber serve --mdns`, the server publishes its peer listener over mDNS (`mdns-sd`, `cyber-app/src/peer_discovery.rs`) as `_cyber._tcp` instance `cyber-<machine>` with TXT `role=peer`, `v=1`, `machine` and `fp`, on the bound address (every IPv4 interface for `0.0.0.0`, never loopback interfaces), republishes the new fingerprint after `invite --rotate-key`, and says goodbye on shutdown; a loopback peer listener, or none, is skipped with the warning `mDNS not published` and its reason. The main API is never published (the old `--mdns` warning about the main bind is gone; the spec delta modifies "mDNS discovery" accordingly). `cyber peers discover [--timeout 3s] [--json]` lists the listeners that answer (machine, addresses, full fingerprint, `added as <remote>` when a remote pins that key, else `not added`) and keeps the result for an hour in `<state>/peers-discovered.json`; `cyber peers add` refuses (exit 1, before any request or storage) a code whose fingerprint differs from the one discovered at the same address (an address, host name or `<machine>.local` on the same port). Discovery adds nothing and grants nothing. Production-path tests: `cyber-cli/tests/peers_cli.rs` `discover_lists_a_published_peer` (two real binaries; scenario "Discover a desktop") and `cyber-app/tests/peers.rs` `mdns_publishes_the_peer_listener_only_off_loopback` (the registered production server with `ServeOptions.mdns`: loopback and missing listeners skipped, publication, rotation, unpublish); both skip their network half with a logged reason where an in-process mDNS probe is not resolved within 5 s. Section 9 (attach) is implemented: `POST /api/v1/peer/attach/:id` (scope `attach`) resolves a Session the grant is shown (as for listings) and raises `remote.attach` in it with resource `peer:<grant>`; only the permission rules decide whether to ask (only an explicit user-level `remote.attach` allow skips the prompt, blanket and repository allows still ask, any `deny` refuses, no Mode approves an attach), the request waits up to 2 minutes, and a refusal, no answer (the request is then withdrawn), a server without an interactive client or unreadable rules fail with the new `PermissionDeniedError` (`403`). A peer that hangs up withdraws its request; refusing an attach neither halts the Session nor declines its other requests, and an approval covers that attach only. An approval returns an attach ID that the other attach routes need in `X-Cyber-Attach` for that Session and grant, so nothing of the Session is sent before it: `GET …/session`, `…/messages`, `…/events?after=` (from now on without `after`), `POST …/prompt` (text and image parts only, as the user), `…/interrupt`, `GET …/permissions` and `POST …/permissions/:request/reply` (never for an attach request); `DELETE /api/v1/peer/attach/:id` closes it, an hour without requests (and no open event stream) ends it, and revoking its grant ends it at once; each of these ends its open event streams too, which also re-read their grant every 2 s. `cyber attach <session>@<peer>` (an enabled remote; `PeerNotConfiguredError` exit 2 otherwise, and a terminal is required) waits for the approval and opens the TUI with the attach transport of `cyber-client` (`Client::attach_peer`, pinned TLS without the 10 s request timeout), which maps the TUI's Session routes onto the attach routes and answers every other route locally as missing; the TUI's snapshot treats a missing inbox or question list as empty. A server URL or local Session after `cyber attach` is still unavailable, and the OpenAPI group `peer` with `PermissionDeniedError` in the SDK guards comes with section 10. Production-path tests: `cyber-app/tests/peers.rs` `attach_through_a_peer_needs_scope_and_approval` (the registered production server's TLS peer listener and the `cyber attach` transport; scenarios "Attach approved", "Attach without approval", "Attach to a peer Session", "Message grant cannot attach"), `cyber-tui` `the_tui_opens_a_peer_session_through_an_attach` (the TUI's start, prompt and snapshot through a real App's peer listener) and `cyber-cli/tests/peers_cli.rs` `attach_needs_a_configured_peer_and_a_terminal` (real binary). Section 10 (surfaces and docs) is implemented. TUI: a received block from another machine reads `✉ from review@laptop · ses_… (its Session there)`, its held card is titled "Held message from another machine", `/messages` has a machine column (the peer, or `local`) and the expanded row says `on <machine> <ses_…>`, and the sidebar shows `Peers  desk ●  nas ○` under the unchanged MCP section, only when the server has enabled remotes; the line reads the new `GET /api/v1/peers/remotes` (`[{ name, online }]`, sharing the listing's 15 s cache and 5 s timeout; OpenAPI group `peers`), and a client attached through a peer, which lacks the route, shows none. `cyber doctor` adds `peers:listener` (off; the configured address and TLS before the server runs; else the running listener's URL, TLS, fingerprint and active grant count, or a warning when `peers.listen` is set but the server has no listener) and `peers:remotes` (the remote names, a warning naming those whose token is in the file fallback). The OpenAPI document lists the 14 peer listener operations under group `peer` (`v1.peer.hello`, `send`, `outcome`, `sessions`, `status`, `attachOpen`…`attachReply`) with the `peerBearer` scheme, `x-cyber-listener: "peer"` and the required `X-Cyber-Attach` header on attach routes; they are not main-API operations, so `cyber api`, JSON-RPC and MCP serve cannot call them there. The TypeScript SDK is regenerated: `client.peer` for a client connected to a peer listener with `auth: { type: "account", token: "cyp_…" }`, `client.peers.remotes()`, and the `isPermissionDeniedError` guard. The README has a "Messaging across machines" section (pairing, the SSH-tunnel setup with a plain loopback listener, security defaults), `docs/features.md` a Direct peers row, and the local core guide's stale `AccountRequiredError` and exit-6 notes are corrected. Production-path tests: `cyber-tui` `screens_peers::messages_between_machines_name_the_machine` (two production Apps over HTTP, desk's TLS peer listener, insta snapshots `peers_held_card_100x30`, `peers_messages_view_100x30`, `peers_sender_sidebar_140x30`; the release from the card, laptop's tracker pass and its Peers line with an offline remote), `cyber-cli/tests/peers_cli.rs` `doctor_reports_the_peer_listener_and_file_tokens` (real binary, before and with the server running), `cyber-server/tests/peers.rs` `every_documented_peer_operation_is_routed_on_the_peer_listener` (every documented peer operation answered by the TLS peer listener, none on the main API) and the SDK's `peers.e2e.test.ts` (the real binary's service with a plain loopback listener; `client.peers.invite`, then `client.peer.hello`, `sessions`, `status` and a refused `attachOpen` with the grant's token). Review fixes (tasks section 12): a peer's `to` resolves only among the Sessions its grant is shown (a hidden Session is `AddressNotFoundError` without its ID; an ambiguous name lists only shown candidates, by workspace name unless `peers.share_paths`); an attached peer's `always` permission reply is refused with `InvalidRequestError`, so no saved approval outlives the attach; the outcome tracker and the remote watch poller ask peers in parallel with one client and at most 5 polls per peer per pass, skip a peer after its first failure in a pass and reschedule a change that cannot be recorded; only a `deliver` pattern naming a peer (with `@`) delivers a cross-machine sender, while `*`, `ses_*`-style patterns and a Session override `deliver` still hold; the peer listener serves HTTP/1.1 only, at most 64 connections, with a 10 s header (and idle) timeout, and refuses an address with 30 failed tokens a minute before any lookup; peer message limits count every Session of a grant together (`peer:<grant>:*`); a peer's `503 ServiceUnavailableError` is reported as such and records nothing; a wildcard listener's default pairing host is this host's LAN name (also mDNS TXT `host`), and `invite` hints at `--host` when `peers.machine` differs. Tests are listed in tasks 12.1–12.8.

Acceptance by hand (2026-10-19, macOS, one host): two real `cyber serve` instances `desk` and `laptop` with separate HOME/XDG config, data and state directories, `peers.listen` on `127.0.0.1:48701`/`48702` with TLS, and a scripted local OpenAI-compatible provider; 14 scenarios, all passing. Observed: `cyber doctor` reports `peers:listener: pass https://127.0.0.1:48701 · TLS on · fingerprint sha256:… · 0 active grants` and `peer-tls/{cert,key}.pem` are mode 0600; `invite` prints the code once and the `peer_grant` row holds only `token_sha256`; `add --token-env` pins the fingerprint and writes `{env:…}`, the raw token appearing 0 times in config; `list`/`test` show the peer online with latency and the receiver's grant as "can receive, cannot reply"; a Session send to `migration@desk` (after the `message.send` ask) is `held` on both sides (`origin_kind` peer, `from review@laptop`) and desk stays idle; releasing in desk's TUI promotes an untrusted `✉ from review@laptop` block and runs a Turn, and laptop's record becomes `released` about 2 s later, or 6 s after laptop restarts when it was down during the release (`messaging.message.released.1` in its history); request fields cannot spoof the sender (`from` unknown field `400`, `from_name: ceo@nas` `400`, `from_name: migration` named `migration@laptop`); `list_sessions` shows `migration@desk` with the workspace name only, and `*@desk` offline after desk stops; a send to a stopped peer fails with `PeerUnreachableError` with `next_poll_ms` NULL and nothing queued; a wrong fingerprint fails `add` (nothing stored), `test`, `send` (no body reaches desk) and curl; `revoke` makes the next send fail in 0.05 s with `PeerAuthError (unauthorized)`; `cyber attach migration@desk` is refused without the `attach` scope, raises `remote.attach` (`peer:laptop`) with it, gives `PermissionDeniedError` on reject and opens desk's Session on approval, a prompt typed on laptop running on desk; `peers discover` with mDNS on `0.0.0.0` lists desk (`added as desk` on laptop, `not added` on a fresh third home whose config stays unchanged); on the peer listener `Origin` is `403`, `/api/v1/sessions` `404`, plain HTTP to the TLS port fails, `cyp_` on the main API `401`, and the 121st request in a minute `429`. Pty captures of both TUIs (held card, release, `/messages` machine column, sidebar `Peers desk ●`, attach rejected/approved/prompt) were saved as HTML, text and PNG.

Bugs found and fixed with regression tests: an attached TUI stayed on "Thinking" because `GET /api/v1/peer/attach/:id/events` without `after` sent no live events (now `server.connected`, then durable and live events of that Session; `crates/cyber-server/src/http/events.rs` `session_instance_events`; test `crates/cyber-app/tests/peers.rs` `attach_through_a_peer_needs_scope_and_approval`); `cyber messages send` and `cyber attach` printed the error tag twice (`commands::tagged`; unit test `tagged_tests::a_message_that_names_its_tag_is_not_prefixed_again` and `peers_cli.rs` `two_binaries_pair_and_test`); a refused `cyber peers add` hinted `cyber peers rm` for a remote never added (`peers_cli.rs` `refuse_another_pin`).

Not demonstrated or open: the OS keyring was mocked with `--token-env` (keyring storage is covered only by `peer_tokens` unit tests against an in-memory vault); two-way pairing with desk's model replying to laptop (task 11.1 asks for it); the SSH-tunnel flow behind a port forward (task 11.2); a run across two physical machines and on Linux or Windows. Known gaps: an attached TUI shows received cross-machine messages as raw `<cross_session_message …>` text (the attach routes do not serve the messaging history the TUI uses); `cyber attach` prints "waiting for … to approve" even when the grant lacks the `attach` scope; after `cyber peers add --token-env` a running server keeps its old environment, so re-pairing needs a restart; the attached TUI's sidebar shows the receiver's full Location path (listings do not).

## Parallel workflow runs (`add-parallel-workflow-runs`, 2026-10-16)

Sections 1–10 of [`add-parallel-workflow-runs`](../../openspec/changes/add-parallel-workflow-runs/tasks.md) are implemented and tested, with production-path tests per section (App over authenticated HTTP with the gated scripted provider, the real `cyber` binary, the SDK against the real binary, and the TUI through `TestBackend` against the production server):

- **Fair agent scheduler** (`runtime/workflow_scheduler.rs`): the `min(workflows.max_concurrent, CPU cores)` agent slots go to the least recently granted run, FIFO within a run, work-conserving; per-run share from `max_concurrent` (CLI, HTTP, resume, `workflow` tool, `meta.max_concurrent`) or `workflows.max_concurrent_per_run`, recorded as `share`. Unit and property tests (300 seeded sequences, R ≤ 8: no idle slot while a run is eligible, each head waiter granted within R grants).
- **Independent lifecycle:** per-run locks for pause, stop, resume, dispatch and end; a Location writer registry for worktree isolation across runs.
- **Run queue** (`runtime/workflow_queue.rs`): `workflows.max_running_runs` (default 8), `queued` with `queue_position`, `workflow.run.dispatched.1`, oldest first; pause and stop of a queued run; wall budget not charged while queued.
- **Recovery of several runs:** running runs become `paused` with `interrupted: true`, queued runs stay queued and are dispatched after recovery; `POST /workflows/runs/resume_interrupted`, `cyber workflows resume --interrupted`, `workflows.resume_after_restart`.
- **Coalesced handback:** up to 8 consecutive workflow summaries share one Turn.
- **Run summaries:** agent counts, tokens, cost, active elapsed time, share and queue position; `status`/`session_id`/`directory`/`limit`/`cursor` filters; `workflow.run.progress` live events (coalesced at 250 ms, only on change).
- **CLI:** the `runs` table, `--all`, `--status`, `--watch` (TTY redraw, line or NDJSON otherwise), `run --max-concurrent`.
- **SDK:** `client.workflows.list()` and `get()`, progress in `events()`, `examples/parallel-workflows.ts`; the e2e tests run against `target/debug/cyber` (`CYBER_BIN`; the CI `sdk` job now builds it).
- **TUI:** the `/workflows` monitor (list, run tree, child transcripts, `p`/`r`/`x`/`R`/`s`/`l`/`a`, per-row nyan bars, TOKENS from 120 columns), the sidebar Workflows section, the `workflows` footer field, aggregate known progress on the Nyan progress line, and run-end notices (`tui.workflow_notifications`). The Nyan indicator itself (`tui.spinner`, `improve-tui-experience` 8.8) and the Stats book comparison line landed in the same tree.

### End-to-end acceptance (manual, real binary and server)

Driven with a gated scripted local provider (no network, no live model) against `cyber serve --register --port 0`, the `cyber` binary, HTTP, and the TUI in a pty read with pyte; `workflows.max_concurrent: 4`, `max_running_runs: 3`. The tooling and captures live in the session scratchpad (`acc-runs/`), not in the repository.

| # | Scenario | Observed |
|---|---|---|
| 1 | Three runs at once: CLI (30 agents), HTTP (3), the `workflow` tool in Session S (3, `max_concurrent: 2`) | All running within 0.26 s, queued A=26, B=3, C=3 while A held the 4 slots. After B and C queued: B started at grants 1, 4, 7, C at 2, 5, 8, A at 3, 6, 9… (gap ≤ R=3; FIFO would have put B behind 26 grants of A). Peak 4 overall, 2 for C; 36 agents, one model call each; results correct; S got its handback |
| 2 | `max_running_runs: 2`, four runs | q3 `#1 in queue`, q4 `#2` (`show`: `queued (#2 in queue)`), no model calls for either; q3 dispatched when q1 completed, q4 when q2 completed; all 2/2 |
| 3 | Pause one, stop another, resume the first | Pause removed Y's 3 queued calls, X and Z unchanged; Y's in-flight agent finished and the freed slot went to Z. Stop cancelled Z's two agents with X's still in flight. Y resumed and completed (one model call per agent), X 4/4, Z stayed stopped |
| 4 | SIGKILL with two runs mid-flight and one queued; restart; `resume --interrupted`; repeated with `resume_after_restart: true` | K1, K2 `paused` (`interrupted`, `the server restarted; resume to continue`), K3 dispatched; `resumed <K1>`, `resumed <K2>`; settled agents never called the model again; each interrupted child Session resumed once; all completed 3/3/2. The automatic variant matched |
| 5 | `cyber workflows runs --watch` | Non-TTY: 53 change lines and 53 NDJSON objects, A from (0,4,26) to (30,0,0); SIGINT exit 0, no run changed. TTY (120×12 pty): redrawn in place, Ctrl-C exit 0 with the run still running |
| 6 | TUI `/workflows` | Four runs with phase, nyan bar, d/r/q/f, tokens, cost, elapsed and `#1 in queue`; run tree (`▼ Fan out 0/30 · 1 running · 29 queued`), child transcript and Esc back; `p` paused only the selected run (`paused (1 settling)`), `p` while settling showed `⚠ Run has agents still exiting; retry shortly` on the row, `p` after it settled resumed; sidebar rows, `⚙ 4 workflows · 1 queued`, `✓ own completed · 6 agents · $0.03 · 28s` in the transcript and a toast for a CLI run; all indicators gone after the last run |
| 7 | Nyan indicator | A held Turn drew the full-width `▀` trail with `~[,,_,,]:3  Thinking · 1.9s · esc interrupt`; a run at 2/6 drew a third of the width and `own 2/6 agents · 33%` |
| 8 | Stats book line | `Your input and output are ~1.1x the tokens in Brave New World` in the Stats tab and in `cyber stats` |

Bugs found and fixed, each with a regression test: Enter/`t` on an agent of a run another Session owns showed `no transcript yet` (`screens_workflows.rs` `the_transcript_of_an_agent_of_another_sessions_run_opens`, fails without the fix); a queued run printed `cost -0.0000 USD` and an agent-less handback carried `"cost_usd": -0.0` (`a_run_without_agents_costs_zero_not_negative_zero`, `handback_of_a_run_without_agents_costs_zero_not_negative_zero`, real-binary `cli.rs` `workflows_run_wait_keeps_waiting_while_the_run_is_queued`); the monitor lacked the TOKENS column the spec requires (added from 120 columns, snapshots updated).

### Automated acceptance (section 11)

Sections 1–10 were committed in `0b784c0`; section 11 and its follow-ups are uncommitted in the working tree.

- **11.1–11.3, real binary and server:** `crates/cyber-cli/tests/parallel_workflows.rs` (in `cyber-cli`, not `cyber-app`, because only `cyber-cli` tests can run the real binary through `CARGO_BIN_EXE_cyber`). It drives the background `cyber serve` the CLI starts, with `workflows.max_concurrent: 4`, `max_running_runs: 3` and the gated scripted local model. `three_runs_share_the_slots_fairly_and_are_controlled_independently` starts A (12 agents, `cyber workflows run`), B (2, HTTP) and C (6, Session S's `workflow` tool, `max_concurrent: 2`) within 1 s, checks the listed `running` status, `queued` counts and shares, and compares every gate opening with a step-by-step model of least-recently-granted scheduling. C never exceeds 2. It then pauses B, stops C, resumes both, queues a fourth run D that is dispatched when A ends, and counts each owner's summaries per attempt. `a_killed_server_recovers_its_runs_and_resume_interrupted_completes_them` and `a_killed_server_resumes_its_runs_after_restart_when_configured` SIGKILL the registered service (`taskkill /F` on Windows) with A and B mid-flight and Q queued. After the restart A and B are `paused` with `interrupted` agents and Q is dispatched. Both runs are resumed, by `cyber workflows resume --interrupted` or automatically with `resume_after_restart: true`, and they complete 3/2/1. Each interrupted child continues once.
- **11.4, TUI:** `crates/cyber-tui/src/screens_parallel.rs` `the_monitor_controls_the_selected_run_of_three_and_notices_their_ends` (`TestBackend` against the production server; `max_concurrent: 3` so the counts match on every runner, since the server caps slots at the core count). `p`, `x` and `r` change only the selected run, which is checked over HTTP and in snapshots `cyber_tui__screens_parallel__*.snap`. The run-end notices appear.
- **Stability:** 5 runs in a row before the last fix and 5 after it, and 3 more during this validation: `parallel_workflows` 3 passed (about 4 s), `screens_parallel` 1 passed (about 2.5 s).
- **Follow-ups (tasks 11.5), each with a regression test:**
  - A notification race in `workflow_watch.rs`: a run that failed before the watch read it notified nobody under `location`. Test: `another_sessions_run_that_ends_before_it_is_read_notifies_the_location`.
  - A queued run's sidebar row read `······ 0/0` and now shows `#<n> in queue`. Test: `a_queued_run_shows_its_queue_position_in_the_sidebar`.
  - Wrapped tool output and user messages lost their gutter (`view/wrap.rs` `wrap_hanging`). In a highlighted `read` card, wrapped rows also continued under the line-number column, so `│ 12 word013…` looked like numbered line 12. They now hang under the code (`view/messages.rs` `number_width`). Test: `screens_markdown::wrapped_user_messages_and_tool_output_keep_their_gutter`, snapshot `wrapped_gutter_80x24`; it fails without the fix.
  - Windows CI runs `cyber-cli --test parallel_workflows` and `cyber-server --test http workflow_run_lists`.
- **Real binary, second manual pass:** three runs (12/2/6 agents, C with `--max-concurrent 2`) started within 0.05 s, and a fourth was queued. `cyber workflows runs` matched `GET /workflows/runs` (alpha 1/1/10/0, beta 0/2/0/0, gamma 2/1/3/0, delta `#1 in queue`). All four completed 12/2/6/2. In a 160×48 pty, the TUI `/workflows` monitor, the sidebar (`delta  #1 in queue`), the `⚙ 4 workflows · 1 queued` footer and a wrapped long `read` showed the fixed layout. The captures are in the session scratchpad (`acc-runs2/`), not in the repository.

### Not done

- Neither change is archived (12.2 and `improve-tui-experience` 10.2), so the ROADMAP inventory refresh is pending.
- Long lines still wrap by character, not by word.
- The Windows paths of the new tests (`taskkill /F`, the Windows CI steps) are type-checked only (windows-gnu clippy with stub mingw tools). They have not run natively.

### Verification (macOS, 2026-10-16)

Whole working tree, after the section 11 tests and doc updates:
- `cargo test --workspace --no-fail-fast`: 3,152 passed, 0 failed, 4 ignored (223 test binaries).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo clippy --target x86_64-pc-windows-gnu --all-targets -p cyber-tui -p cyber-cli -- -D warnings`: clean (type-check only).
- `cargo fmt --all -- --check`: clean.
- `python3 scripts/generate_sdk.py --check`, `npm run typecheck` and `npm test`: passed (92 tests, 90 passed and 2 skipped without `CYBER_BIN`).
- `openspec validate --all --strict`: passed (66 items).
- `python3 scripts/spec_lint.py`: 0 errors and 21 warnings (the baseline).

## Provider-reported usage limits (`improve-tui-experience` section 9, 2026-10-16)

Uncommitted, part of the TUI overhaul working tree (see the P1 status entry of the same date for the whole-tree validation). `cyber-llm/src/limits.rs` reads plan and rate-limit windows from provider response headers (Anthropic unified and rate-limit headers, `x-codex-*`, `x-ratelimit-*`), never from local usage; `GET /api/v1/usage/limits` (`v1.usage.limits`, SDK `client.usage.limits()`) lists the latest reading per provider and window, and the TUI Usage tab draws one meter per window with `d`/`w`. A provider without such headers yields an empty list and no meters (tested through the App router in `crates/cyber-app/tests/usage_limits.rs`). Not driven against a live provider.

## M2.1 Workflow runtime, increment 1 of 4: engine, discovery, config and surface stubs (2026-10-15)

### OpenSpec

The change `implement-m2-1-workflow-runtime` has a proposal, design, tasks and spec deltas:

- The proposal maps all 26 P2 `workflows` requirements, plus the related requirements in background-tasks, agents-subagents, worktrees, permissions-modes, observability-costs, server-api, client-sdk, cli-commands, exec-mode, hooks, skills-commands, tui, compaction, goals, storage-events, session-runtime and provider-catalog, either to tasks across four increments or to an out-of-scope list:
  - M2.2 bundled workflows
  - automatic orchestration and `/orchestrate`
  - the M2.3 `goal` option
  - P3 runners and remote agents
  - P4 sharing
- The deltas resolve the real conflicts:
  - TypeScript is deferred; `.ts` files are listed as `UnsupportedSyntax`.
  - The sandbox limits are specified as charged synchronous JS time, with ranges and the extra removed globals.
  - `WorkflowReplayMismatch` is added to Diagnostics, and forged kinds are reported as `ExecutionFailure`.
  - The tier value format is defined.
  - The monitor stop key is `x`, as in tui.
  - Workflow worktrees are named `run-<ulid>-<index>` on `cyber/run-<ulid>/<index>`, because `run_<id>-<index>` violated the managed-name pattern.

`openspec validate --all --strict` passes for 63 items. `spec_lint.py` reports 0 errors and the same 21 warnings as before.

The brief named the commands `run/list/status/resume/cancel`. They map onto the spec's `cyber workflows run/list/show/resume/stop`; the spec also has `runs`, `pause` and `logs`.

### Engine (`crates/cyber-workflow`)

**JS engine.** The engine is rquickjs, pinned to `=0.14.0` with default features off.

- License: MIT.
- Pre-generated bindings cover every release target (darwin, linux gnu and musl, windows msvc and gnu).
- The C code builds through `cc`, as with `rusqlite` `bundled`.
- New transitive packages: `rquickjs-core`, `rquickjs-sys` and `relative-path` (MIT OR Apache-2.0).
- boa was rejected because it has no heap limit or interrupt handler.

**Meta parser.** A hand-written tokenizer is used instead of oxc, because of disk and build weight. It extracts a pure-literal `meta` and detects `import`, `export … from` and `require`.

**Per-run sandbox.** Each run gets its own Runtime and Context:

- **Heap limit.** Exhaustion is reported as `ExecutionFailure: memory limit of N MiB exceeded`.
- **CPU limit.** An interrupt handler charges only synchronous JS slices, measured with `Instant`, against `script_cpu_seconds`. Exceeding it gives `Timeout`. Time awaiting host calls is not charged.
- **No module loader.**
- **Frozen prelude.**
  - `Date.now`, `Math.random`, `performance.now`, `Date()` and argument-less `new Date()` throw the determinism message. `new Date(0)` still works.
  - `eval`, `Function`, the Function-constructor back doors, timers, `WebAssembly`, `WeakRef`, `FinalizationRegistry`, `SharedArrayBuffer` and `Atomics` are removed.
  - The host API globals (`agent`, `ask`, `checkpoint`, `phase`, `log`, `parallel`, `pipeline`, `args`, `ctx`) are non-writable. `args` and `ctx` are deep-frozen.

**Scheduler.** A cooperative scheduler runs host futures through the `WorkflowHost` trait. Host errors carry a `kind` that scripts can catch but cannot forge. Agent options are validated against the documented keys. The log is capped at 1 MiB with a marker. A promise that can never settle is detected, and at most 10,000 calls may be pending.

**Diagnostics.** A typed `Diagnostic` covers all 11 kinds and carries `line:col`.

**Discovery.** Saved Workflows are discovered from:

1. `<config>/workflows`
2. each `.cyber/workflows` from the project root down to the Location

A later root overrides an earlier one by `meta.name`, and the replaced paths are recorded as `shadowed`. Traversal is bounded, never follows links, and reads files of at most 1 MiB. Inline scripts load through `WorkflowModule::inline`.

### Config (`cyber-core`)

`config/workflows.rs` validates the `workflows` section during config load, replacing the opaque key:

| Key | Default | Range or values |
|---|---|---|
| `max_concurrent` | 16 | 1–256; the effective value is `min(config, cores)` |
| `max_memory_mb` | 256 | 16–4096 |
| `script_cpu_seconds` | 60 | 1–3600 |
| `tiers` | defaults `fast`/`smart`/`verify` | name pattern `^[a-z][a-z0-9_-]{0,31}$`; value `provider/model` or `role:<role>` |
| `auto` | `suggest` | `off`, `suggest`, `on` |
| `auto_threshold_files` | 10 | 1–100,000 |
| `size_guideline` | 10 | 1–1000 |

Unknown keys are rejected.

### Production surfaces

- **CLI.** `cyber workflows list` (table and `--format json`) discovers saved Workflows for the Location and honours `CYBER_DISABLE_PROJECT_CONFIG`. `run`, `runs`, `show`, `pause`, `resume`, `stop` and `logs` are parsed and exit 2 with "`cyber workflows <sub>` is not available in this build yet".
- **HTTP.** `ANY /api/v1/workflows` and `/api/v1/workflows/{*rest}` answer `503 {"_tag":"ServiceUnavailableError","service":"workflow"}` behind the normal origin and authentication guard. Before this change they returned an untagged 404 even without credentials. These routes are not in OpenAPI yet, so the SDK is unchanged.
- **CI.** Two native Windows steps, "Native workflow engine and runs" and "Native workflow agents, permissions and surfaces", run the `cyber-workflow`, CLI, server and app workflow tests.

### Tests (red first where noted)

**`cyber-workflow`:** 16 unit tests and 13 engine tests that use a fake host.

Unit tests:

- tokenizer: strings, templates and regexes
- pure-literal meta and the offending-token positions
- forbidden imports with specifier and position
- discovery: override, `.ts`, parse errors and links (Unix)

Engine tests:

- agents, phases, logs and checkpoints reaching the host
- top-level await
- frozen globals
- undefined capabilities (`fetch is not defined` with its line)
- determinism guards
- busy-loop and post-await `Timeout`, and a slow host that is not charged
- 16 MiB memory limit
- `ParseError` and thrown-error positions
- catchable `AgentFailed`, a forged kind reported as `ExecutionFailure`, and `ask` reported as `NoInteractiveUser`
- invalid options, a promise that never settles and a cyclic return value
- `parallel` order, concurrency and settle mode
- a pipeline whose second stage starts before slow first-stage items finish
- log truncation

These tests were written together with the module, so they have no recorded red run.

**`cyber-core` `config`:** `workflow_settings_have_defaults_and_validate_ranges_and_tiers`. It was red first (the types were missing).

**`cyber-cli` `cli`** (red first, against "`cyber workflows` is not available"):

- `workflows_list_shows_saved_workflows_with_project_overrides_and_parse_errors` runs the real binary in a temp repository with global and project modules. It expects names, scopes and descriptions, the project override, `computed.js: ParseError: meta must be a pure literal (2:29)`, and the JSON shape.
- `workflow_run_subcommands_are_explicitly_unavailable`.
- `unshipped_command_is_explicitly_unavailable` now uses `goals`.

**`cyber-app` `workflows`** (red first, an untagged 404): `workflow_routes_answer_service_unavailable_naming_the_group` uses the real App router over authenticated TCP. It expects 401 without credentials and 503 with `service: workflow` for GET, POST and DELETE on the group.

### Verification (macOS)

Results:

- **Pass:**
  - `cargo fmt --check`.
  - Native clippy with `-D warnings` for `cyber-workflow`, `cyber-core`, `cyber-cli`, `cyber-server` and `cyber-app` (all targets).
  - `x86_64-pc-windows-gnu` clippy for the same crates (all targets, stub MinGW CC/AR, type-check only).
  - `cyber-workflow` (29 tests), `cyber-core` `config` (51) and `ci_contracts`.
  - `cyber-cli`: every test binary except `review_command` (below).
  - `cyber-server` `http` (62).
  - `cyber-app` `workflows`.
- **Pass in isolation only:** `cyber-app` `server` passed 42 of 43. `public_worktree_creation_streams_setup_and_retains_failed_session` hit its 10 s setup-event timeout under full parallel load, and passes alone.
- **Flaky under parallel load:** `cyber-cli` `review_command`. One or two tests fail intermittently because of their 2 s billing-refresh and 3 s review timeouts. The suite passes with `--test-threads=1`. A `main` worktree that shared the target directory also failed in 2 of 8 parallel runs, against 6 of 9 on this branch, so the flake predates this change. The runs overwrote each other's `target/debug/cyber`, so these numbers are only indicative.

### Status and next increments

M2.1 is **in progress**. Increment 1 delivers static loading and a sandboxed engine but does not execute runs. The next increments are:

- **Increment 2: durable model and resume.**
  - `run_` aggregate, events, migration and projections.
  - Replay keys and revisions, and `WorkflowReplayMismatch`.
  - Pause, stop and recovery; shutdown marks runs `paused`.
- **Increment 3: real agents and run policy.**
  - Real child-Session agents through a `ToolHost` workflow method.
  - Schema validation, concurrency cap, budgets, tiers, write isolation, permissions and Mode ceiling, `ask`, read-only `tools`, and result handback.
- **Increment 4: surfaces.**
  - Server routes, OpenAPI and SDK.
  - Full CLI with `--wait` exit codes, `cyber exec --workflow`, the `workflow` tool, and the `/workflows` TUI monitor.

Reserved budget enforcement is still refused by `crates/cyber-server/src/runtime/budget.rs`. It is a P2 exit criterion but does not block M2.1, so it is tracked as follow-up task 4.16.

Re-verified on 2026-10-15 after the interrupted run: `cargo test --locked` passes for `cyber-workflow` (29), `cyber-core` `config` (51) and `ci_contracts`, `cyber-cli` `cli` (43), `cyber-server` `http` (62) and `cyber-app` `workflows`. Native and windows-gnu clippy with `-D warnings` and `openspec validate --all --strict` (63 items) also pass. An unrelated `io-extras` `windows-sys` re-resolution in `Cargo.lock` was reverted, so the lockfile only adds the new crate and the rquickjs packages.

Native Windows runs of the new tests are pending CI.

## M2.1 Workflow runtime, increment 2 of 4: runtime core (2026-10-16)

Increment 2 delivers what the brief named "runtime core": real agents, the global limit, tiers, and runs started over HTTP. It comes ahead of resume, which the original task plan had placed in this increment. `tasks.md` section 6 lists what was delivered; tasks 3.2–3.4, 4.3, 4.7 and 5.1 are annotated as partly done.

### What runs now

- **HTTP.** `POST /api/v1/workflows/runs` takes `{session_id, script | name, args}` and answers `201` with the run. `GET /api/v1/workflows/runs[?session_id=]` lists runs, and `GET /api/v1/workflows/runs/{id}` shows a run with its settled agents. Every run has an owner Session, whose Location, Mode and rules apply to the run. Runs execute in the server, independent of the client. Other workflow paths still answer `503` with `service: workflow`.
- **Engine thread.** Each run drives the QuickJS engine on its own thread. Agent calls cross to the tokio runtime, and dropping a pending call cancels its child.
- **Agents.** `ToolHost::spawn_workflow_agent` is unsupported by default. `BuiltinHost` implements it next to `subtask_request` by synthesizing an `agent` invocation with `operation_key = <run_id>:<call_key>`, and `AppHost` forwards it (the forwarding guard test covers this).
  - `agent.rs` now spawns every child through one path keyed by `SpawnSource` (`Model`, `User`, `Workflow`).
  - Each child Session carries `workflow_run_id`, uses its `agt_` ID as subagent name, and runs under the owner's Mode as a ceiling. Deny rules apply.
  - The result is `{ text, session_id, cost, tokens }`, or the validated object for a `schema` after a single result retry.
  - Failures are typed: `AgentFailed`, `SchemaMismatch`, `PermissionDenied` and `ModelUnavailable`.
  - `goal`, `tools`, `budget`, `isolation: "worktree"`, `checkpoint()` and `ask()` reject explicitly as not available.
- **Concurrency.** One FIFO semaphore per runtime, `min(workflows.max_concurrent, cores)`, is set by `App::build`. The permit travels into `ChildGuard`, which releases it only after the child settles. `agents.max_concurrent` does not apply to workflow agents.
- **Tiers.** `fast`, `smart`, `verify` and custom tiers resolve per Location. `tier_role_ref` falls back to the default model, so `verify` works without an advisor. An unknown tier is `ModelUnavailable`.
- **Durability.** The `run_` aggregate (migration `20261016000000_workflow_runs`) has the events `workflow.run.{started,completed,failed}.1`, `workflow.agent.settled.1`, `workflow.phase.1` and `workflow.log.1`, projected into `workflow_run` and `workflow_agent`.
  - The settled event is appended before the script sees the result.
  - A failed append ends the run through `WorkflowHost::fatal`, and failed phase or log records fail it directly.
  - Call keys are `sha256(prompt + canonical options)` plus an ordinal.
- **Model.** `SessionInfo.workflow_run_id` is new, so `sdk/openapi.json` and the generated TS types were regenerated. The workflow routes are still outside OpenAPI (increment 4).

### Tests

- **`cyber-workflow` engine:** 3 new tests (19 engine tests in total):
  - `parallel_group_concurrency_bounds_calls_in_flight`
  - `a_fatal_host_failure_ends_the_run_before_the_script_sees_the_result`
  - `a_failed_phase_or_log_record_fails_the_run`

  Phase appending and log truncation are covered by the existing tests.
- **`cyber-llm` roles:** `tier_roles_fall_back_to_the_default_model` and `roles_parse_from_their_keys`.
- **`cyber-app` services:** tier resolution through `model_roles` (with `verify` falling back to the default), and limits from the `workflows` section.
- **`cyber-server` `workflow_runs`** (fake agent host, 6 tests):
  - the settled record is stored before a dependent call starts (ordering assertion), with the exact event sequence
  - `agt_` IDs, call keys, owner Mode and phases
  - ordinals and failed records
  - the global limit of 2 is never exceeded across two runs, and is FIFO (`a3` starts before any `b*`)
  - tiers and `ModelUnavailable`
  - unavailable options reject before any agent starts
  - a host without workflow agents fails closed, and a missing owner is refused

  Writing this suite exposed that an `async move` block captures only the fields it uses. The fake host must bind the permit explicitly; `BuiltinHost` moves the whole request.
- **`cyber-tools` `workflow_agents`** (`BuiltinHost` with scripted models, 4 tests):
  - children carry `workflow_run_id` and the `agt_` name and inherit the `plan` Mode
  - a missing result is retried once and then reported as `SchemaMismatch`
  - a provider error is reported as `AgentFailed` and a deny rule as `PermissionDenied`
  - with `agents.max_concurrent: 1`, three workflow agents still stream concurrently (a rendezvous adapter)
- **`cyber-app` `workflows`** (production App, AppHost and AppServices over authenticated TCP, with a local OpenAI-compatible scripted model, 3 tests):
  - An inline `parallel([agent(a), agent(b, {schema})])` script, started with `POST /workflows/runs`, completes when polled.
  - It returns the text result and the schema-validated object.
  - Both child Sessions report `workflow_run_id`, `parent_id` and the `agt_` name over `GET /sessions/{id}`.
  - The model saw exactly one plain request and one request offering `return_result`.
  - Script failures with a location, catchable `ModelUnavailable`, and `400` (diagnostic), `404` and `401` errors are covered, and unimplemented paths still answer `503`.
- **CI:** the native Windows step now also runs `cyber-server --test workflow_runs` and `cyber-tools --test workflow_agents`.

### Not done in this increment

- Resume and replay, pause and stop, recovery, and shutdown marking runs `paused`.
- `workflow.call.recorded.1`, checkpoints and `ask()`.
- Run budgets, `max_agents` and reserved enforcement (4.16).
- The `workflow.run` permission, and auto-rejecting agent asks in non-interactive runs (4.12). Today an agent's permission ask waits for an interactive client like any child's.
- `CYBER_WORKFLOW_RUN_ID` in tool processes.
- Write isolation, read-only `tools`, warm-up, and result handback to the owner Session.
- OpenAPI and the SDK run handle, the CLI run commands, and the TUI monitor.


### Verification (macOS, 2026-10-16)

- **Pass:**
  - `cargo fmt --check`.
  - Native clippy with `-D warnings` (all targets) for `cyber-workflow`, `cyber-server`, `cyber-tools`, `cyber-app`, `cyber-llm`, `cyber-store`, `cyber-core` and `cyber-cli`.
  - `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR, all targets) for `cyber-workflow`, `cyber-server`, `cyber-tools`, `cyber-app` and `cyber-llm`.
  - Full test suites of `cyber-server`, `cyber-store`, `cyber-workflow`, `cyber-llm`, `cyber-tools`, `cyber-app` and `cyber-cli`.
  - `openapi_document_is_current` and `generate_sdk.py --check`.
  - `openspec validate --all --strict` (63 items).
- **Stack overflow found and fixed.** `cyber-tools` `bundled_skills::bundled_batch_load_can_delegate_five_real_isolated_coding_tasks` overflowed its stack after the shared spawn path was introduced, because the larger spawn future was nested inline. `run_with_skill` and `run_workflow` now box it, and the existing test is the regression guard.
- **Test flake found and fixed.** The FIFO test was flaky at first: two permits were released together, so the order in which agents recorded their start was a race. Staggered releases make the grant order observable, and the test then passed 8 of 8 repeated runs.

## M2.1 Workflow runtime, increment 3 of 4: durable replay and resume, lifecycle, run budgets (2026-10-16)

### What runs now

- **Replay** (`runtime/workflow_replay.rs`).
  - Every host call gets an issue sequence number per revision. The call key is `sha256(prompt + canonical options)` for an agent and the name for a checkpoint, plus an ordinal.
  - `workflow_call` (migration `20261017000000_workflow_budget`) stores each call's key, its agent, its outcome and the order the script received it.
  - A relaunch of the same revision delivers the recorded results in that order without calling the host. Because the script is deterministic, it reaches the same state.
  - A call issued during that prefix with a different key, or a recorded result whose call is never issued, fails the run with `WorkflowReplayMismatch`. The engine only polls host futures while the script is idle, so the check is exact.
  - The prefix ends at the first recorded failure, so a resume retries a failed agent.
  - After the prefix, and in a new revision, calls are matched by key: completed results and checkpoint values are reused, an unsettled agent resumes its existing child Session, and other calls run anew.
  - Logs and phases of the replayed part are not recorded again.
- **Revisions.** Each revision stores its script source, its digest and a configuration digest (limits and tiers) in `workflow_run_revision`. Inline scripts are relaunched from the stored source, and saved Workflows are read again. A changed digest without `revision: true` is refused with `409 WorkflowReplayMismatch`.
- **Resume of unsettled agents.** The request carries `resume: true`, so `BuiltinHost` passes `resume: <agt_ID>` through the shared `agent.rs` path (`resolve_subagent`, `claim_child_execution`, `resume_child`) with a continuation prompt. A child whose admission a stop closed is reopened first: `reopen_subtree` with the stop receipt. The child's next Drain runs `recover`, which reconciles interrupted tool calls through `ToolHost::reconcile` before any further mutation.
- **Lifecycle** (`runtime/workflow_control.rs`).
  - The events `workflow.run.{paused,resumed,revised,stopped}.1` join `started`, `completed` and `failed`, together with `workflow.agent.started.1` and `workflow.call.recorded.1`.
  - A process-local `Control` per attempt (like `jobs.rs` `Control`) holds the signal (running, pausing, stopping, closing) and counts leases: the engine thread and every agent task.
  - Pause is recorded at once; in-flight agents finish and are stored, and the next `agent()` ends the attempt once nothing is in flight.
  - Stop cancels in-flight agents and calls `stop_subtree` for each unsettled child, keeping the stop receipts. A cancelled agent is never recorded as settled, so a resume continues it.
  - Resume is refused with `Run has agents still exiting; retry shortly` while a lease is held or an agent child's Drain is running.
- **Server shutdown and recovery.** `Runtime::shutdown` durably pauses running runs (agents `interrupted`) before admission closes, then waits (bounded) for engines and agent tasks after the Drains settle, before `tools.shutdown`. `App::build` calls `recover_workflow_runs` next to `recover_jobs`, and it pauses runs that a crashed process left `running`. The native Windows service path runs the same `Runtime::shutdown` and `App::build`.
- **Run budgets.**
  - `RunBudget` (`cyber-core`) layers `budgets.run`, `meta.budget` and the request's `budget`, with defaults `max_agents` 200 and `max_wall_seconds` 14400. `reserved` is refused.
  - `workflow_run_budget` joins `runtime/budget.rs` scopes through the `workflow_run_id` of the Session or any ancestor (now a `session` column, backfilled from `session.created.1`). The existing check before every child provider Turn therefore enforces the run's cost, token, Turn and wall limits. Wall time excludes paused segments.
  - Before a new agent starts, `check_run` applies the same limits plus `max_agents` and records `budget.exceeded.1` on the run (signals in `workflow_run_budget_signal`).
  - The call rejects with `BudgetExceeded: <limit>`, and the run ends `budget_exceeded`, keeping a partial result. A child refused by the run budget also rejects with `BudgetExceeded`.
  - `ctx.budget` is a live getter that the engine refreshes before every JavaScript slice: `{ spent_tokens, spent_cost_usd, agents_started, remaining }`.
- **Usage.** `GET /api/v1/usage?scope=run&id=` returns the run's total and its rollups per agent, model and tier (`RunUsageReport`). `UsageReport.scope` and `RunUsageReport.scope` are literal discriminants of the `ScopeUsage` union (OpenAPI and TS SDK regenerated).
- **HTTP.** `POST /workflows/runs/{id}/pause|resume|stop` and `GET /workflows/runs/{id}/logs`. Resume takes `{revision?, budget?}`, and start takes `budget`. Lifecycle conflicts answer `409`.

### Tests

- **`cyber-workflow` engine:** `ctx_budget_is_a_live_frozen_host_snapshot`.
- **`cyber-core`:** `run_budgets_layer_config_meta_and_invocation_with_defaults`.
- **`cyber-server` replay units (5):**
  - the prefix follows delivery order and stops at the first failure
  - recorded results are delivered in recorded order
  - a changed or reordered call is a mismatch
  - a recorded result whose call is never issued is a mismatch
  - notes of the repeated part are not recorded again
- **`cyber-server` `workflow_runs` (8 new, fake agent host plus real child Sessions on the scripted model):**
  - pause lets the in-flight agent finish and starts none, and resume replays `a`, the checkpoint and `b` from the store, so only `c` reaches the host and logs and phases are not repeated
  - a changed script without a revision is refused, while a new revision reuses `a` by key, returns the recorded checkpoint value and runs only the edited call
  - stop cancels agents and records no settlement; resume is refused while a stubborn agent still holds its task, then succeeds (eventual state) and resumes both agents under their original IDs
  - a settlement that cannot be recorded fails the run closed
  - `max_agents: 1` rejects the second agent, gives `budget_exceeded` and records `budget.exceeded.1` on the run
  - `ctx.budget` reflects real child spend, and wall time is enforced
  - a run cost cap refuses a real child's second provider Turn, and `run_usage` reports it
  - shutdown and startup recovery leave the run paused, and a new runtime over the same store resumes the interrupted agent
- **`cyber-app` `workflows` (production App over authenticated TCP, 4 new):**
  - A three-agent sequential run, whose scripted model holds agent 3's request, is restarted on the same database file. It comes back `paused` with agent 3 `interrupted`. `POST …/resume` completes it: the model got no new request for agents 1 and 2, agent 3 continued its existing child Session (the same child set), and `…/logs` holds the single log entry.
  - `POST …/stop` over HTTP stops the real child's subtree. Resume answers `409` until the agent exits, then reopens the child's admission (stop receipt before `session.admission.reopened.1`) and completes.
  - `budget: {max_agents: 1}` over HTTP ends `budget_exceeded`, and a `reserved` budget answers `400`.
  - A mixed `fast`/`verify` run reports per-model cost through `GET /usage?scope=run` (the verify model priced 10× costs 5× two fast agents) and per-tier rollups.
  - `scripted_model.rs` now serves each connection on its own thread and can hold a request.
- **`cyber-app` `windows_service`:** a native service stop pauses a running run whose agent waits for a slot, and a rebuilt App keeps it paused. Only type-checked on macOS (windows-gnu clippy); the native Windows CI step runs it.
- **`cyber-server` `http`:** `scope=run` for a missing run answers `404`, and other unavailable scopes still answer `503`.
- **TS SDK:** `usage.get narrows a Workflow Run rollup by its scope`.

### Not done in this increment

- `ask()` and `waiting_input` (reserved), read-only `tools`, warm-up, worktree isolation, reserved budget enforcement (4.16), the `workflow.run` permission, and `CYBER_WORKFLOW_RUN_ID`.
- Workflow routes in OpenAPI, the SDK run handle, live run events on the event stream, CLI run commands and the TUI monitor (increment 4).
- `ctx.budget` reads live spend. A script that branches on it can diverge on replay, which fails closed with `WorkflowReplayMismatch`.

### Verification (macOS, 2026-10-16)

- **Pass:**
  - `cargo fmt --check`.
  - Native clippy with `-D warnings` (all targets) for `cyber-server`, `cyber-app`, `cyber-workflow`, `cyber-core`, `cyber-tools`, `cyber-store` and `cyber-cli`.
  - `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR, all targets, including `windows_service.rs`) for `cyber-server`, `cyber-app`, `cyber-workflow` and `cyber-tools`.
  - Full test suites of `cyber-server`, `cyber-store`, `cyber-workflow`, `cyber-app`, `cyber-tools` and `cyber-core`, plus `cyber-cli --test cli workflow`.
  - TS SDK `npm test` and `typecheck`, `openapi_document_is_current` and `generate_sdk.py --check`.
- **Flake (not this change).** `cyber-app` `mcp_stdio::local_call_timeout_sends_cancelled` failed once under full-suite load ("was never published"). It passed alone twice and in the next full run.

## M2.1 Workflow runtime, increment 4 of 4: surfaces and safety (2026-10-16)

### What landed

- **Permissions.** `permissions::defaults` has an explicit `workflow.run * ask` rule with source `default`, which `evaluate` reports. `auto` allows only that default rule without the classifier; `bypass` allows; a configured rule (for example a deny on one workflow name) wins in every Mode. The `workflow` tool's approval carries `name`, `description`, `phases`, `budget`, `origin` and `digest`.
- **Unattended agents.** A run records `interactive`. Runs started over the API or the CLI are not interactive: `requests.rs` resolves any ask of a Session that belongs to such a run (its own `workflow_run_id` or an ancestor's) as `Unattended` at once, and the tool reports `No interactive approver for workflow run`. A run that cannot be read counts as not interactive.
- **Write isolation.** `ToolHost::workflow_agent_writes` (BuiltinHost: git checkout plus an edit tool in the profile's tool set; AppHost forwards it, guarded by `app_host_forwards_every_tool_host_method`). Agent calls without an `isolation` choice are counted when issued, so a `parallel()` fan-out is counted before its first agent starts. A new agent gets `RunWorktree` (`run-<ulid>-<index>` on `cyber/run-<ulid>/<index>`, index = start ordinal) when another writer is in flight; `isolation: "worktree"` always isolates and `"none"` opts out. `create_child_worktree` takes the run naming, authorizes deny-only and keeps the checkout (`Cleanup::Keep`). Results and `workflow_agent` records carry `branch`.
- **Run results.** `workflow_results.rs`: JSON up to 1 MiB stays inline, larger values go to `<state>/workflows/<run_id>.result.json` with a 16 KiB preview (a missing state directory fails the run closed). Every ended run (completed, failed, budget-exceeded, stopped) fires `WorkflowRunEnd`, then admits one queued summary `msg_workflow_<run_id>` with source `workflow` (waking the owner only for tool-started runs) and records `workflow.run.notified.1`. Startup recovery admits unrecorded summaries again; an existing message with that ID counts as admitted.
- **`workflow` tool.** `tools/workflow.rs` resolves the module and budget through the shared `runtime::workflow_start` rules (also used by HTTP), asks `workflow.run`, starts the run as `RunStarter::Session` and returns the handle. Inline scripts are kept at `<state>/workflows/<run_id>.js` (`script_file`) by the runtime, for HTTP runs too. Workflow agents cannot start runs. `action_of("workflow")` is `workflow.run`, so a full deny hides the tool.
- **CLI.** `cyber workflows run/runs/show/pause/resume/stop/logs` use `cyber-client` against the shared server. `run` creates the owner Session (Mode `--mode`, else `default`) and sends a saved name or a file's source; `--max-cost/--max-turns/--max-tokens/--timeout/--max-agents` become budget fields. `--wait` polls and exits 0/1/4/130 (Ctrl-C stops the run). `--runner` exits 2 as unavailable before any server starts.
- **API and SDK.** Seven `v1.workflow.*` operations in `openapi.rs` with JsonSchema on the run types (`schemars` added to `cyber-workflow` for `Diagnostic`); `sdk/openapi.json` and the TS sources regenerated. `sdk/typescript/src/workflows.ts` adds `client.workflows.run()` with `events()`, `status()`, `pause()`, `resume()`, `cancel()` and `result()` (rejecting with `WorkflowRunError`), plus `examples/run-workflow.ts`.
- **Live events.** Run events are published on the bus through `RunLog`; `event_location.rs` maps them to the directory in `workflow.run.started.1`, so they stream on `GET /api/v1/event` for that Location.
- **Hooks.** `HookIdentity.workflow_run_id` (reserved, `run_` prefix) comes from the Session envelope for workflow agents, and from the run for `WorkflowRunStart`/`WorkflowRunEnd` (runtime lifecycle events on the owner) and for a workflow agent's SubagentStart/SubagentStop. **Bug fixed:** SubagentStart used to pass `workflow_run_id` as a payload field, which the envelope now reserves; it moved into the envelope (regression: `cyber-app` `server::application_emits_runtime_lifecycle_hooks_with_session_identity` and the workflow hooks test).

### Tests (production path first)

- **`cyber-cli` `cli::workflows_run_against_the_shared_server_and_wait_maps_exit_codes`:** the real `cyber` binary starts the registered shared server with a scripted local OpenAI-compatible model. `workflows run x --wait` exits 0 with progress and the result, a throwing `boom.js` exits 1, `--max-agents 0` exits 4, `--args` that is not an object exits 2, `runs`/`show`/`logs` print the runs, resuming a completed run exits 1, and `--runner` exits 2.
- **`cyber-app` `workflows` (production App, 4 new):** the model's `workflow` tool in `default` Mode asks with name, phases and budget before any run exists, then the run completes, its script is kept under `state/workflows`, and the owner gets exactly one `msg_workflow_<run_id>` entry and answers it; a deny rule refuses the tool in `bypass` and keeps nothing; an API run's agent `bash` ask is rejected with the exact message while the instance SSE stream delivers the run's `workflow.*` events with the run's Location; (unix) `WorkflowRunStart`, `WorkflowRunEnd`, the agent's SubagentStart and its `PreToolUse` all carry `workflow_run_id`.
- **`cyber-tools` `workflow_agents` (2 new):** with `Fixture::native_git()` (explicit full access on Windows only), two parallel writers get distinct worktrees on `cyber/run-<id>/1` and `/2` reported in their results, a lone writer and `isolation: "none"` stay in the checkout, and the worktrees are kept; the `workflow` tool starts an owned run (golden `goldens/workflow.txt`), is denied by a name rule in `bypass`, and is refused inside a workflow agent.
- **`cyber-server`:** `workflow_runs` (3 new: isolation decisions and numbering with a fake writer host; large result to a file with the summary admitted once and again once after a simulated restart; API runs are not interactive), `workflow_results` units (inline vs file, handback text), `workflow_start` units (moved from `cyber-app`, plus budget layering).
- **Units:** permission engine `workflow_run_asks_by_default_with_a_visible_source_and_is_allowed_in_auto_and_bypass`; `cyber-core` `hook_envelopes::workflow_agent_envelopes_carry_the_run_and_payloads_cannot_forge_it`; CLI exit-code mapping.
- **TS SDK:** `test/workflows.test.ts` (3) against the generated types; `npm test` (86), `typecheck` and `build` pass.
- **Windows:** the native job's workflow step adds the server lib, permission, envelope and CLI unit tests to the existing workflow suites.

### Verification (macOS, 2026-10-16)

- `cargo fmt --check`; native clippy `-D warnings --all-targets` for `cyber-core`, `cyber-server`, `cyber-tools`, `cyber-app`, `cyber-cli` and `cyber-workflow`; `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR) for the same crates.
- Full test suites of those crates; the only failures in the first full run were the SubagentStart bug above and suites that need the `cyber-sandbox-exec` helper, which `cargo clean` had removed (they pass after `cargo build -p cyber-sandbox --bins`).
- `openapi_document_is_current`, `generate_sdk.py --check`, `cargo metadata --locked` and `openspec validate --all --strict` (64 items).

### Still open after M2.1

`ask()`/`waiting_input`, direct read-only `tools`, prompt-cache warm-up, `CYBER_WORKFLOW_RUN_ID`, compaction reporting, the TUI monitor and Save action, `exec --workflow`, Job integration, saved-Workflow listing over HTTP and capability discovery, reserved budgets (4.16), M2.2 bundled Workflows, P3 runners.

## M2.1 Workflow runtime: gaps (2026-10-22)

Change `implement-m2-1-workflow-runtime`, tasks 4.3, 4.8, 4.9, 4.11, 4.15, 5.1, 5.4, 5.5, 5.6 and 5.7; M2.2 tasks 2.1 and 3.1–3.3 with them.

- **`CYBER_WORKFLOW_RUN_ID`** in every sandboxed tool process of a workflow agent (and its subagents) and in hook commands of a run, read from the store without the Session lock. Test: `workflow_agents::a_workflow_agents_shell_sees_its_run_id`.
- **Workflow library over HTTP.** `GET /api/v1/workflows` (`v1.workflow.library`), `client.workflows.library()`, `workflow` in `/health` `features`. Test: `workflows::the_library_lists_saved_workflows_by_precedence_with_issues`.
- **Direct read-only tools.** `meta.tools` and the invocation's `tools`, enforced by the engine; `ToolHost::workflow_tool` with the built-in checks; results recorded as `tool` calls and replayed. Tests: `engine::direct_tools_obey_the_meta_and_invocation_allowlists`, `workflow_runs::direct_tool_results_are_recorded_and_replayed_after_a_pause`, `workflow_agents::direct_tools_return_structured_results_and_refuse_unattended_asks`.
- **`ask()`.** `runtime/workflow_ask.rs`, `workflow.run.waiting.1`/`workflow.run.answered.1`, `args.answers`. Tests: `workflow_agents::ask_waits_for_the_owner_or_uses_declared_answers`, `workflow_agents::stopping_a_waiting_run_withdraws_its_question`.
- **`cyber exec --workflow`.** Test: `cli::exec_runs_a_workflow_and_maps_its_end_to_exit_codes`.
- **`/tasks` and `/stop`.** Test: `screens_tasks::tasks_lists_workflow_runs_and_stop_all_stops_them`.
- **Save.** `POST /workflows/runs/{id}/save`, `w` in the monitor. Test: `workflows::an_inline_runs_script_is_saved_into_the_project_workflows`.
- **Compaction reporting.** `compactions` per agent, `⟲N` in the monitor. Test: `workflow_runs::a_run_reports_its_agents_compactions`.
- **Prompt-cache warm-up.** `runtime/workflow_warmup.rs`, `workflows.prompt_cache_warmup`. Test: `workflow_agents::a_fan_out_sharing_a_prefix_waits_for_the_first_response`.

Still open: reserved budgets (4.16) and finished-transcript retention (5.9, which needs storage retention). Session export of run summaries is done by `add-session-export`, and goal deferral while runs are active (5.8) by the goals change.

## M2.1 Workflow runtime: review fixes

Fourteen review findings were checked; thirteen were real (two of them, the HTTP and CLI `workflow.run` check, were one bug). Each fix has a regression test that fails without it.

- **Replay of parallel batches (blocker).** `RunHost::agent`/`checkpoint` now take their sequence number when the engine dispatches the call, not when the future is first polled, so every call of a `Promise.all` batch is issued before any is served. Before, a batch whose results arrived out of order failed its resume with `WorkflowReplayMismatch`. Test: `workflow_runs::a_parallel_batch_settled_out_of_order_resumes_through_replay`; unit `workflow_replay::a_batch_issued_together_replays_in_recorded_order_whatever_is_polled_first`.
- **Rejected calls are sequenced.** Option errors, unknown tiers, invalid payloads (new `WorkflowHost::rejected`) and `ask()` are issued and recorded as `workflow.call.recorded.1` errors, so a script that catches them replays without a false mismatch. Test: `workflow_runs::a_caught_rejection_beside_a_parallel_agent_replays_without_a_mismatch`.
- **Pause stops queued agents.** The global FIFO permit is taken before `workflow.agent.started.1`; a call that waited for it while the run paused or stopped starts nothing and stays unrecorded for the resume. Test: `workflow_runs::agents_queued_behind_the_global_limit_do_not_start_after_a_pause`.
- **Lifecycle race.** `finish()` records the script's end under `workflow_lifecycle` and only while the run is still `running` or `paused`, so a stop never coexists with `completed` and a pause never overwrites a completed run's result. Test: `workflow_runs::pause_and_stop_never_overwrite_or_duplicate_the_scripts_own_end` (20 rounds each).
- **`workflow.run` over HTTP and the CLI.** `Services::authorize_workflow_run` (AppServices: `BuiltinHost::workflow_run_denial` over the owner Session's rules) refuses a deny for the bare name or the qualified resource with `403 PermissionDenied`, on start and on resume; `ask` counts as approved for an authenticated client. Tests: `cyber-app` `workflows::http_starts_and_resumes_honour_workflow_run_deny_rules`, `cyber-cli` `cli::a_workflow_run_deny_rule_refuses_cyber_workflows_run`.
- **Origin-qualified resource.** `permissions::workflow_resource`: `<name>` (global saved), `project:<name>`, `inline:<name>@<digest>`; the tool checks the bare-name deny first (`Ctx::refuse_denied`). An allow or "always" approval no longer covers another script declaring the same name. Tests: unit `a_workflow_allow_rule_or_approval_covers_only_its_origin`; the `cyber-app` tool test now expects `inline:tooled@sha256:…`.
- **`ctx.budget` replay.** The snapshot is recorded at the revision start (`run.started`/`run.revised`) and with each delivered result (`budget_snapshot`), and replayed as recorded; it no longer reads live spend between results. Test: `workflow_runs::ctx_budget_replays_as_recorded`.
- **Mode ceiling on resume.** `launch` uses the stricter of `run.mode` and the owner's current Mode. Tests: `workflow_runs::a_resume_keeps_the_mode_the_run_started_with_as_a_ceiling`, `cyber-app` `workflows::a_resumed_run_keeps_its_starting_mode_as_the_agents_ceiling`.
- **Summary after resume.** A resume clears `notified`, `result_file` and `result_preview`; the summary of the attempt after the `n`th resume is `msg_workflow_<run_id>_<n>`. Tests: `workflow_runs::a_resumed_run_hands_back_again_and_drops_an_earlier_result_file`, `cyber-app` `workflows::a_tool_started_run_that_is_resumed_hands_back_each_end_and_drops_a_stale_result_file`.
- **`meta` nesting (blocker).** The literal reader stops at 64 levels with `ParseError`, so a deep literal can no longer abort the server through the tool, HTTP or discovery. Tests: `meta::deep_nesting_fails_without_overflowing_the_stack`, `discovery::deeply_nested_meta_is_an_issue_not_a_crash`, `engine::deeply_nested_scripts_fail_without_crashing`.
- **Error description charged (blocker).** `run_jobs` and `outcome` describe errors inside the charged slice, so `throw { toString() { for (;;) {} } }` ends with `Timeout`. Test: `engine::describing_a_hostile_error_is_charged_to_the_script`.
- **Bounded native bridge.** `Bridge` enforces 10,000 pending calls, payload and phase bytes within `max_memory_mb`, and the 1 MiB log cap as the script calls it; a refused call throws `ExecutionFailure` synchronously. Tests: unit `the_bridge_bounds_queued_logs_calls_and_bytes`, `engine::synchronous_call_loops_are_bounded_by_the_bridge`.
- **Ctrl-C in `run --wait`.** One `ctrl_c` listener covers the poll and the sleep. Test (unix): `cli::ctrl_c_between_polls_stops_a_waited_run` (failed with the old loop).

Spec delta and `local-core-guide.md` updated (Permissions for runs, Run results, Run lifecycle states, Run budgets, Replay of workflow host calls, module format and sandbox). `openspec validate implement-m2-1-workflow-runtime --strict` passes.

## M2.1 Workflow runtime: review fixes, round 2

Seven findings were checked; all seven were real. Each fix has a regression test, and the replay and slot tests were confirmed to fail with the fix reverted.

- **Failed resumed agents stayed "unsettled".** `Replay::load` cleared a call key's unsettled `agt_` only on a completed settlement, so after revision 2 resumed revision 1's interrupted agent and it failed, every relaunch resumed the failed child again (with the interrupted-attempt prompt) and skipped `charge_agent`. `index_call` now clears it on any settlement. Test: `workflow_runs::an_agent_that_failed_when_resumed_is_retried_as_a_new_agent`.
- **Pause while queued for a global slot.** `RunHost::slot` raced the FIFO acquire only against cancellation, so a run paused while waiting kept its place in the queue, held its engine lease and refused resume with `AGENTS_EXITING` until another run freed a slot. The acquire now also races `signalled()` (signal leaves `Running`), drops the queued acquire and halts through `admit()`. Test: `workflow_runs::a_run_paused_while_queued_for_a_slot_leaves_the_queue_at_once`.
- **Budgets could be widened by the model or a script.** `RunBudget::resolve` let `meta.budget` and the invocation replace fields, and `null` removed a cap. Configuration and the defaults are now a ceiling (`tighten`: lower only, `null` for a set limit refused); `RunBudget::overridden` keeps the explicit raise for the authenticated client (HTTP start via `workflow_start::client_budget`, resume). Tests: unit `budget::run_budgets_layer_config_meta_and_invocation_with_defaults`, `workflow_start::run_budgets_layer_configuration_meta_and_invocation`, `cyber-app` `workflows::the_workflow_tool_cannot_lift_configured_run_budgets`.
- **Unbounded host-call records and no wall clock without agents.** `RunHost::append` charges every record against `MAX_RUN_RECORDS` (100,000) and `MAX_RUN_RECORD_BYTES` (256 MiB), seeded from the run's stored events; a checkpoint value over 1 MiB fails the run closed; past `max_wall_seconds` the first non-agent call rejects with `BudgetExceeded` and the next fails the run closed (status `budget_exceeded`). Tests: unit `workflow_host::records_are_bounded_by_count_and_bytes`, `workflow_runs::a_checkpoint_loop_ends_at_the_wall_budget_and_large_checkpoints_fail_closed`.
- **Unbounded phases.** The bridge charges each queued entry its text plus `size_of::<Op>()`, does not queue a repeat of the last queued phase and refuses titles over 200 characters; the engine skips a phase equal to the current one and fails past 256 distinct titles (`MAX_PHASES`), as does `RunHost::phase` across revisions; `workflow.phase.1` carries `phases` only when a title is added. Tests: unit `tiny_phases_are_charged_per_entry_and_repeats_are_not_queued`, `engine::phase_loops_are_bounded_and_repeats_do_not_reach_the_host`, `workflow_runs::phase_loops_record_changes_only_and_stop_at_the_title_cap`.
- **Nested runs over the API.** `Runtime::start_workflow_run` refuses an owner that is, or descends from, a workflow agent (`409`, `WORKFLOW_AGENT_STARTS_RUN`), failing closed when the ancestry cannot be read (`workflow_run_of`); the tool uses the same fail-closed lookup. Tests: `workflow_runs::a_workflow_agent_session_cannot_own_a_run`, `cyber-app` `workflows::a_workflow_agent_session_cannot_own_a_run_over_http`.
- **Edited `script_file` ignored on resume.** HTTP resume of an inline run reads `script_file` (stored source only when the file is gone), then re-runs `authorize_workflow_run` on it; an edit without `revision: true` answers `409 WorkflowReplayMismatch`. Test: `cyber-app` `workflows::an_edited_script_file_relaunches_as_a_new_revision`.

Spec delta (sandbox, Phases, logs and checkpoints, Permissions for runs, Model-authored workflows, Run lifecycle states, Run budgets, Replay) and `local-core-guide.md` updated.

## M2.1 Workflow runtime: end-to-end acceptance with the real binary (2026-10-16)

The debug `cyber` binary was exercised in a temporary git project with its own `HOME` and `CYBER_HOME`, against a scripted local OpenAI-compatible stub (Python, `127.0.0.1` only, logging each request with start and end times). No live model was used. `workflows.max_concurrent` was 2.

| # | Scenario | Command | Observed | Result |
|---|---|---|---|---|
| 1 | Saved fan-out | `cyber workflows list`; `cyber workflows run fanout --wait` (`phase`, `parallel` of 3 schema `agent()` calls, `pipeline`, `log`) | Listed as `fanout [project]`. Exit 0, result `{"total":140,"ranked":[…]}` built from the three schema results. Stub times: two requests overlapped and the third started only after both ended (cap 2). Each agent is a child Session (`v1.session.children`) with `workflow_run_id`, its `agt_` name and its own `cyber/run-<id>/<n>` worktree. `logs` prints `analyzed 3` and the ranking. | pass |
| 2 | Inline run over HTTP | `cyber api POST /api/v1/workflows/runs` with `session_id` and `script`, then `GET /api/v1/workflows/runs/{id}` | Answer `running`; polls show agents `running`, then `completed` with `{"joined":"answer for one \| answer for two"}`. | pass |
| 3 | Determinism guards | `cyber workflows run <file>.js --wait` with `Date.now()`, `Math.random()`, `import fs`, `require('fs')`, `fetch`, `process` | `ExecutionFailure: non-deterministic API; pass values through args` (exit 1) for the clock and random calls; `UnsupportedSyntax: import of "fs" …` / `require of "fs" …` with `line:col` (exit 2, nothing started); `fetch is not defined` and `process is not defined` (exit 1). The stub's request count was unchanged (5 before, 5 after), so no agent ran. | pass |
| 4 | Resume after a crash | `kill -9` of the server while agent 2 of 2 was in flight, `cyber service start`, `cyber workflows resume <id>` | Before the fix the resumed agent failed: `Subagent execution ownership is held or requires recovery`. After it: the run is `paused` with agent 2 `interrupted`, the resume completes, agent 1 is replayed with no new request for its prompt, and agent 2 continues its existing child ("Your previous attempt … was interrupted"), 1 new request in total. A graceful `cyber service stop` behaved the same before and after. | pass (after fix) |
| 5 | Budget | `cyber workflows run seq --max-tokens 50 --wait`; same for `fanout` | `budget_exceeded`, exit 4, `BudgetExceeded: max_tokens`; the sequential run made 1 model request before the next `agent()` was rejected. In the parallel run the agents in flight are now listed `cancelled` (they stayed `running` before the fix). | pass (after fixes) |
| 6 | Stop | `cyber workflows stop <id>` on a run of held agents | `stopped`; the agents are `cancelled`, their child Sessions end "Provider turn interrupted", and the third agent (queued behind the cap) never starts. | pass |
| 7 | Permissions and Mode ceiling | deny rule `workflow.run: {fanout: deny}`; `cyber --mode plan/default/accept-edits workflows run writer/basher --wait`; `cyber exec "…"` with a model that calls the `workflow` tool | The CLI and HTTP start answer `Permission denied: workflow.run is denied for fanout` (exit 1, no model request) while `seq` still runs. In `plan` the agent's `write` gets "Plan mode is read-only"; in `default`, `write` and `bash` get "No interactive approver for workflow run"; in `accept-edits` `write` succeeds and `bash` is still rejected. Child Sessions carry the run's Mode. `cyber exec` (dont-ask) denies the `workflow` tool ("Not pre-approved"), and `--mode auto` starts the run. | pass |

### Bugs found and fixed (each with a regression test that fails without the fix)

- **Crash left workflow agents unresumable.** A server killed while it owned a child left its `child.execution.changed.1` claim `held` forever, so resuming the run failed the agent. Claims now record the registration ID of the server that holds `server.lock` (`Runtime::adopt_server_lock`, called by `run_server_until` once the lock is acquired). The next lock holder releases claims an earlier server left held; claims without a holder (runtimes that never took the lock) stay held. Tests: `cyber-server` `child_ownership::the_next_server_lock_holder_releases_only_claims_of_an_exited_server`; `cyber-cli` `cli::a_run_resumes_after_the_server_is_killed_mid_agent` (unix, real binary, `kill -KILL`).
- **Agents stayed `running` after their run ended.** When a script ended (completed, or failed, for example on a `BudgetExceeded` rejection inside `parallel`) with agents in flight, those agents were cancelled but their records stayed `running`. `workflow.run.completed.1` and `workflow.run.failed.1` now mark them `cancelled`, as stop does. Test: `workflow_runs::agents_in_flight_when_the_script_ends_are_recorded_cancelled`.
- **`run --wait` progress.** Agents were tracked by count, so an agent settling before an earlier-listed one was printed twice and the other never; a phase change was printed before the agents of the previous phase that settled in the same poll. Progress is now tracked by agent ID and ordered by phase. Test: `commands::workflows::tests::progress_prints_each_settled_agent_once_in_phase_order`.
- **Doubled error kind.** `error: BudgetExceeded: BudgetExceeded: max_tokens` is now `error: BudgetExceeded: max_tokens`. Test: `commands::workflows::tests::run_errors_name_their_kind_once`.

### Noted, not changed

- `cyber exec --mode default` waits for an approval reply when the model needs one (here the `workflow` tool or `bash`), which looks hung in text mode. This is exec-mode behaviour that is not specific to workflows. Exec's default `dont-ask` and `auto` behave as specified.
- `cyber sessions` is not shipped; child Sessions were inspected through `cyber api v1.session.children`.

### Verification (macOS, 2026-10-16)

- `cargo fmt --check`; native clippy `-D warnings --all-targets` and `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR) for `cyber-server`, `cyber-app` and `cyber-cli`.
- Full test suites of `cyber-server` (40 test binaries), `cyber-app` and `cyber-cli` (396 tests) pass.
- `openspec validate --all --strict` (64 items).

### Whole working tree validation (macOS, 2026-10-16)

- `cargo test --workspace --no-fail-fast`: 2755 passed, 0 failed, 4 ignored.
- `cargo clippy --workspace --all-targets -- -D warnings` and `x86_64-pc-windows-gnu` clippy (stub MinGW CC/AR) for `cyber-app`, `cyber-cli`, `cyber-core`, `cyber-server`, `cyber-store`, `cyber-tools`, `cyber-workflow` and `cyber-llm`: clean.
- `cargo fmt --all -- --check`, `generate_sdk.py --check`, SDK `npm run typecheck` and `npm test` (86 tests), `openspec validate --all --strict` (64 items), `spec_lint.py` (0 errors, 21 warnings, the baseline) and `spec_inventory.py` (no ROADMAP drift).

### Not part of M2.1

- `CYBER.md` instruction files ship in the separate change `add-cyber-md` (loading through the baseline, nested on-read discovery and `cyber doctor`, from one ordered list of file names in `crates/cyber-server/src/runtime/instructions.rs`; evidence in [P1 status](p1-status.md#2026-10-16-cybermd-project-instructions-add-cyber-md)).
