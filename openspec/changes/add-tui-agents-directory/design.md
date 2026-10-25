## Context

- **Listing route.** `GET /api/v1/messaging/sessions?from=&directory=` (`crates/cyber-server/src/http/messaging.rs`) returns `{data:[ReachableSession]}`. Each entry is `{address, session_id, name, kind, state, location{directory, workspace?}, canReceive}`.
  - `kind` on the wire is `subagent`, `local` or `peer`. The spec text says `remote`.
  - `reachable()` (`runtime/messaging/address.rs`) only ever produces `Subagent` or `Local`. Team members are therefore listed as `local`, named by their title.
  - Peers are queried in parallel with a 5-second timeout and cached for 15 seconds. An unreachable peer is one entry: `*@peer`, `offline`, `canReceive: false`.
  - A `directory` filter drops peer entries.
  - The listing returns at most 200 local entries and 50 per peer, with no cursor.
- **Action routes.** These already exist:
  - `POST /messaging/send {from, to, text, notify_when_idle?}`. It returns 200 with `outcome` `delivered`, `held` or `refused`.
  - `POST /messaging/watch {from, target}` returns 201.
  - `PATCH /sessions/{id} {title}`.
  - `POST /collaboration/messages {target:{kind:"direct",participant:{kind,id}}, text, session_id, project, operation_id}`.
- **Hub roster.** `collaboration.rs` reads `/collaboration/status` and, only when it is enabled and bound, `/collaboration/participants`, which returns `{members:[Member], agents:[Agent]}`.
  - Hub agents are not in the messaging address space. They are messaged only through the hub, always as the signed-in human.
- **TUI patterns to copy:**
  - Overlays keep their state on `App`, as `/messages`, `/team` and `/collab` do.
  - `collaboration.rs` owns its fetch, with a generation counter and a 5-second poll.
  - `messaging.rs` keeps the selection by stable ID across reloads.
  - `team.rs` uses Enter → `Action::Open(id)`.
  - `rename.rs` and the Session picker use `Action::Rename{id,title}`.
  - `messaging::clean()` strips control and bidi characters.
- **Names already taken:**
  - `Action::LoadAgents`, `Msg::Agents` and `GET /api/v1/agents` are agent definitions for @-mentions.
  - `mouse::List::Agents` is the Workflow monitor's agent list.
  - `/agent` (alias `/subagents`) is the subagent thread switcher.

## Goals / Non-goals

- **Goals:**
  - One overlay that answers "who can this Session message right now, by what address, and in what state".
  - From that overlay, message, watch, open, rename or review held messages without leaving the TUI.
  - Sources that a later transport can add to without touching rows or actions.
- **Non-goals:** see the proposal. In short: Relay, the P2 Agent view, new transports, bulk sends, and attaching to peers in-process.

## Decisions

### Sources

- **`agents::Source` is an enum** with `Messaging` and `Hub`. Each source has its own loader, returning `Result<Vec<Entry>, SourceError>`, and its own last result on `Directory`.
  - The directory's rows are the concatenation of the sources' rows, in source order.
  - A Relay source later adds one variant and one loader. The P3 Relay entries already arrive through the messaging listing as `<session>@<machine>`, so it may need nothing at all.
- **Messaging source:** `GET /messaging/sessions?from=<attached Session id>`, through `client.at(&session.directory)`.
  - It never passes `directory`, so peer entries are kept, as the CLI does.
  - `kind` is read leniently. `subagent`, `teammate`, `local` and `peer` are accepted, `remote` is read as `peer`, and an unknown kind is shown verbatim (cleaned) as `other`. The spec says `remote` and the code says `peer`; the TUI accepts both rather than deciding that here.
- **Hub source:** `GET /collaboration/status?session_id=`. When `enabled` is not true or no project is bound, the source is `Inactive` and contributes no rows and no error.
  - Otherwise it reads `GET /collaboration/participants?session_id=&project=`. Rows come from `agents` that are `active`, and from `members` that are `active` and are not the current member.
  - It reads only these two routes, not messages or tasks. That keeps it cheap enough for the poll.
  - Its parsing reuses `collaboration::rows`, the array-or-wrapper reader.
- **Why the listing gains provenance (server side).** The user-facing kinds `teammate` and `workflow agent` cannot be derived reliably in the TUI:
  - Team rosters are per team.
  - Workflow agents' child Sessions show up as subagents of the owner.
  - `reachable()` already joins the `session` table, which has `workflow_run_id`, and the team tier already knows the caller's team.

  **Confirmed (task 1.1):** a workflow agent's Session is created by the `agent` tool with `parent_id` set to the Session that runs the Workflow (`cyber-tools` `tools/agent.rs`), and the runtime refuses `workflow_run_id` without a parent. So workflow agents are always listed as children of the run's owner, and `local` with `workflowRunId` cannot occur.

  Tagging there makes the CLI, the MCP `cyber_message_list` and the model's `list_sessions` agree with the spec ("Reachable session listing" already lists kind `teammate`). It is additive: a new optional field, and a kind value the spec already names. This is the added `cross-session-messaging` requirement. The address of a teammate becomes its member name only when that name resolves to it from the caller, which the team tier, resolved first, guarantees.

### Row model

- **`agents::Entry`** has these fields:
  - `key`: a stable selection key. It is `session_id` for listing rows and `hub:agent:<id>` / `hub:member:<id>` for hub rows.
  - `source`
  - `kind`: `Subagent | WorkflowAgent | Teammate | Session | Peer | HubAgent | HubMember | Other(String)`.
  - `address`: the display address, cleaned.
  - `target`: what actions send to (see sanitising). It is `Option`, and `None` disables sending.
  - `name`
  - `state`: `Idle | Running | WaitingInput | Offline | Online | Unknown`.
  - `can_receive`
  - `location`: `Local(dir) | Peer(name) | Hub(project)`.
  - `session_id`: `Some` only for local rows. This is what open and rename use.
  - For hub rows, the `participant` `{kind,id}`.
- **Kind mapping:**
  - `subagent` with `workflowRunId` → `workflow agent`
  - `subagent` → `subagent`
  - `teammate` → `teammate`
  - `local` → `session` (`local` with `workflowRunId` cannot occur, because workflow agents are never roots)
  - `peer` / `remote` → `peer`
  - hub `agents[]` → `hub agent` (state `online` when `online` is true, else `offline`)
  - hub `members[]` → `hub member`, with the role and a `guest` marker in the name column.
- **Location:**
  - A local row shows the directory, shortened from the home directory and middle-truncated.
  - A peer row shows the peer name, taken from the part after the last `@` in `session_id`, and the peer's directory or workspace name when one was shared.
  - A hub row shows `hub:<project>`.
- **Grouping and order:** three groups with headers: **This server**, **Peers** and **Project hub**. Within a group, rows keep the server's order (most recently updated first). An empty group is not drawn.
- **Selection** is kept by `key` across reloads, filters and source refreshes, as `Messages::select` does. When the selected row disappears, the selection moves to the row at the same index, clamped.

### Refresh

- **On open:** both sources load concurrently, and each row group shows `loading…` until its source answers.
- **While open:** every 10 seconds, from `on_tick`. The server already caches peers for 15 seconds, so this costs one local query. The poll is skipped while the compose or rename editor is open, as `collaboration_poll` does.
- **After an action** that changes the roster or state (rename, send, watch): the listing is refreshed once.
- **Stale replies are dropped.** `Request{generation, source}`, and the reply carries `session_id` and `generation`. A reply for another Session or an older generation is dropped. `forget_session_views` resets the directory and closes the overlay on a Session switch.
- **There is no event-driven refresh.** The listing has no change stream. Polling the existing route keeps "no new transport".

### Actions and keybindings

Printable characters edit the filter (filter-as-you-type). Actions therefore sit on Enter and Ctrl chords, which the hint lines list.

| Key | Action | Applies to |
|---|---|---|
| ↑/↓, PgUp/PgDn, Home/End | move the selection | all |
| typing, Backspace, Ctrl+U | edit or clear the filter | all |
| Tab / Shift+Tab | cycle the kind filter `All → Sessions → Subagents → Teammates → Peers → Hub` | all |
| Enter | message: open the compose editor on the row | rows with a `target` and `can_receive` |
| Ctrl+W | watch | listing rows that are not offline |
| Ctrl+O | open: switch the TUI to the Session (`Action::Open`); on a peer row, show `cyber attach <session>@<peer>` in a toast | local and peer rows |
| Ctrl+R | rename: inline title editor; on Enter `Action::Rename{id,title}` | local Sessions and subagents |
| Ctrl+G | held messages: open `/messages` with the `held` filter, selecting the newest held message from this row's address when one exists | listing rows |
| Ctrl+L | refresh now | all |
| Esc | clear the filter when it is non-empty, otherwise close | all |

- An action that does not apply to the row shows a one-line reason in the status line and does nothing. For example: `peer rows cannot be renamed here`, or `refused: this Session cannot receive`.
- **Compose editor:** a multi-line editor drawn over the bottom of the overlay, copied from `collaboration::Editor`, with a 16 KiB UTF-8 limit and paste support. Enter sends and Shift+Enter (or Alt+Enter) adds a newline. Ctrl+N toggles `notify_when_idle` for listing rows. Esc cancels.
  - A **listing row** is sent with `POST /messaging/send {from: <attached>, to: target, text, notify_when_idle}`, with an `Idempotency-Key` per compose. The editor closes and a toast shows `delivered`, `held` or `refused: <reason>` with the message ID.
  - A **409** `AmbiguousAddressError` shows the candidates and keeps the editor open. A 404, 413, 502 or 503 shows the error message.
  - A **hub row** is sent with `POST /collaboration/messages`, as a direct message to the participant, exactly as `/collab` People `d` sends it. It needs a writable membership (`owner` or `member`); a viewer gets `viewers cannot message`.
  - The server's existing rules apply unchanged: inbound rules, rate limits, the chain limit and the peer send permission. The directory adds no permission and does not bypass one.
- **Watch** sends `POST /messaging/watch {from, target}`. A toast shows `watching <address> until <time>`. A 429 `watch_quota` shows the quota message.
- **Rename** issues the same `PATCH /sessions/{id} {title}` that `/rename` and the Session picker's `Action::Rename` send, as a directory operation (`Op::Rename`). `Action::Rename` itself is not reused because its reply reloads the Session list and opens the Session picker over the directory. An empty title is refused locally; the editor starts from the current title for a `session` row. Renaming the attached Session itself stays `/rename`, because it is never in the listing.
- **Idempotency:** each compose sends one request, and the client gives every mutation a fresh `Idempotency-Key` that its transport retries keep, so a retried send is replayed rather than sent twice. Hub sends carry an `operation_id` made once per send.

### Empty, error and disabled states

- **Messaging source:**
  - Messaging disabled for the Location does not fail the listing: the server lists the Sessions with `canReceive: false`, so every row is `closed` and Enter says why. A **503** from the listing (as the send route answers when disabled) gives the group line `messaging is disabled here (messaging.enabled)`.
  - A **404 or 501** (an older server) gives `this server does not list reachable Sessions`.
  - **Another error** keeps the last rows and appends the line `could not refresh: <error>`. A row from a dead connection stays honest because its state is the state as of the last load.
- **Hub source:**
  - An inactive hub draws no group.
  - A failure clears the hub rows and shows `project hub: <error>`, so a revoked membership does not leave a stale roster on screen (as `collaboration_reply` does).
- **Both sources empty:** the overlay says `No other Session can be reached from <name>.` and adds two hints: `cyber peers add` for peers and `/collab` for the hub.
- **Filter with no match:** `No match for "<filter>"` with the active kind filter, and Esc clears it.
- **Offline peers** (`*@peer`) are listed with state `offline`. Only Ctrl+O (the attach hint) applies to them.

### Untrusted text sanitising

Peer and hub text comes from other machines and other people. Their `name`, `address`, `location.directory`, `workspace`, peer name, hub `name`, `role` and project are all untrusted.

- **Every displayed string goes through `messaging::clean()`**, which strips C0/C1 control characters and bidi overrides or isolates. It is then truncated to the column width with `truncate_end`. It is never interpreted as markup, a key or a link.
- **The send target is the raw wire `address` only when `clean(address) == address`.** Otherwise it falls back to `session_id` under the same rule. When both fail, the row is shown with a `⚠` marker and `target = None`. A crafted name can therefore never make the directory send to something other than what is displayed.
- **Peer rows are always marked as peer** (kind `peer`, location `peer:<name>`), whatever their `name` claims. A peer entry cannot pass as a local Session: `session_id` for open and rename is set only for listing rows whose kind is not `peer` and whose `session_id` starts with `ses_` and contains no `@`.
- **Hub IDs** are sent back only as the `participant.id` the hub gave. Hub names are never used as addresses.

### Rendering

- The overlay copies `view::messaging::view`:
  - a full-screen Clear and background,
  - a rounded border with the accent title `Agents · <attached name>`,
  - the kind filter chips right-aligned in the title,
  - the filter line `/ <text>` under the title,
  - the grouped body windowed through `app.mouse.window(List::Directory, …)`,
  - a two-line key hint.
- **Columns:** state glyph, address, kind, state word, `open`/`closed`, Location. On a narrow terminal (under 100 columns), Location is dropped first, then kind.
- **State glyphs**, always followed by the state word:
  - `running`: `●`, info
  - `idle`: `○`, muted
  - `waiting_input`: `!`, warning
  - `offline`: `○`, subtle
  - `online`: `●`, success

  Under NO_COLOR, the glyphs and words carry the state, and the selected row is reverse video. Only theme tokens are used.

## Risks / Trade-offs

- **`/agents` next to `cyber agents`:** `/agents` (this directory) and the planned P2 `cyber agents` / Ctrl+A Agent view share a word for two different views. The ROADMAP names this overlay `/agents`. The Agent view keeps its CLI entry and Ctrl+A, and the help text distinguishes them as "reachable from this Session" and "everything the server runs". If the Agent view later wants `/agents`, the directory can become a tab in it.
- **Polling cost:** one local SQL query (with a 200-row limit) and two hub reads every 10 seconds while open. This is negligible. Peers are served from the 15-second cache.
- **Teammate address change:** after this change, `list_sessions` reports a teammate as `api` rather than `adhoc/api`. Both still resolve, so existing addresses keep working. This only affects callers in a team with the `teams` feature on.
- **Hub sends are human-authored.** A message to a hub agent from `/agents` is authored by the signed-in account, not by the Session. This matches `/collab`, and the compose title says `as <account>` for hub rows.
