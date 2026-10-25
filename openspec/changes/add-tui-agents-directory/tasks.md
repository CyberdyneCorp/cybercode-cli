## 1. Listing provenance (server)

- [x] 1.1 In `crates/cyber-server/src/runtime/messaging/address.rs` `reachable()`, select `workflow_run_id` and carry it on `Target`. Add `#[serde(rename = "workflowRunId", skip_serializing_if = "Option::is_none")] workflow_run_id: Option<String>` to `ReachableSession` (`runtime/messaging/mod.rs`) and set it in `describe`. First confirm how workflow agent Sessions are parented: if they are not children of the owner, record where they appear in the listing in design.md. Unit test: a child Session with `workflow_run_id` carries it, and a plain subagent has none.
- [x] 1.2 Tag the caller's team members with `AddressKind::Teammate` in `reachable()`, reusing the team tier lookup. Their address is the member name when `resolve` maps it to that Session, else unchanged. Unit tests: a lead sees `api` as `teammate`/`api`; a non-member caller still sees `local`/title. (The team tier reads `team_member` rows and is not gated on the `teams` feature, so a caller in no team is the `teams`-off case.)
- [x] 1.3 Production-path test (written in `crates/cyber-app/tests/messaging.rs`, over the production App's HTTP routes, next to the existing listing test): `reachable_listing_tags_teammates_and_workflow_agents`. Covers both "Reachable session provenance" scenarios, including that sending to the listed teammate address delivers.
- [x] 1.4 Regenerate the OpenAPI document (`UPDATE_OPENAPI=1 cargo test -p cyber-server --test http openapi_document_is_current`) and the TypeScript SDK (`scripts/generate_sdk.py`). Check that `cyber messages list --reachable --format json` passes the new field through and that the table's `kind` column fits `teammate`.

## 2. Directory model and loading (`crates/cyber-tui/src/agents.rs`, new)

- [x] 2.1 Add `Source { Messaging, Hub }`, `Entry` (key, source, kind, address, target, name, state, can_receive, location, session_id, participant), `Kind`, `State` and `Location`. Add `Entry::from_listing(&Value)`, which reads `peer`/`remote`, unknown kinds as `Other`, and `workflowRunId` → `WorkflowAgent`, and `Entry::from_hub(agents, members, current_member, project)`. Apply `messaging::clean()` to every string. Compute `target` (the raw address only if it is clean, else a clean `session_id`, else `None`) and `session_id` (only for non-peer `ses_` IDs without `@`).
- [x] 2.2 Add `Directory { sources: per-source {rows, error, loading}, generation, last_poll, selected: Option<String>, filter: String, kind: KindFilter, editor: Option<Editor> }`. Add `visible()` (grouped and filtered), `select(delta)` keeping the key, and `forget()`.
- [x] 2.3 Add `Request { generation, source }` and `async fn perform(client, session, request) -> Msg::Directory { session_id, generation, source, result }`. Messaging calls `GET /messaging/sessions?from=` and maps 503 → `Disabled` and 404/501 → `Unsupported`. Hub calls `/collaboration/status`, then `/collaboration/participants` only when enabled and bound; otherwise the source is `Inactive`.
- [x] 2.4 Add `impl App { open_agents, agents_request, agents_reply, agents_poll, agents_key, agents_paste }`. The reply drops stale Sessions and generations. A messaging error keeps the last rows; a hub error clears its rows. The poll runs every 10 seconds while the overlay is open and no editor is open.
- [x] 2.5 Unit tests in `agents.rs`:
  - parsing of a listing JSON with every kind, plus `remote` and an unknown kind;
  - a `*@peer` offline entry;
  - bidi, ANSI and control characters stripped, with `target` falling back to the ID and to `None`;
  - a peer row never gets a `session_id`;
  - filter on address, name, kind and Location, and the kind filter cycle;
  - selection kept by key across a reload that reorders and removes rows;
  - a stale generation dropped;
  - a hub source that is not enabled contributes no rows and no error.

## 3. Wiring

- [x] 3.1 `app.rs`:
  - add `Overlay::Agents`, `pub agents: agents::Directory`, `Action::Directory(agents::Request)`, `Action::DirectorySend{..}` and `Action::DirectoryWatch{..}` (or one `Action::Directory` with an operation enum);
  - add the `"agents"` slash arm, the `"agents_view"` arm in `run_binding`, and the key dispatch arm;
  - reset in `forget_session_views`.
- [x] 3.2 `perform.rs`: add `Msg::Directory` and `Msg::DirectoryDone{ result }` and route them through `scheduled()`.
  - **Send:** `POST /messaging/send {from, to, text, notify_when_idle}`, with an `Idempotency-Key` per compose. Hub rows go to `POST /collaboration/messages` (reusing the `/collab` direct-message body).
  - **Watch:** `POST /messaging/watch {from, target}`.
  - **Rename:** reuse `Action::Rename`.

  `runner.rs`: apply both messages and call `agents_poll` from `on_tick`.
- [x] 3.3 `palette.rs` BUILTINS: `("agents", "Sessions this one can message: message, watch, open, rename", Some("agents_view"), false)`. `keys.rs` DEFAULTS: `("global", "agents_view", "none", "reachable Sessions directory")`. Add the help rows in `view/overlays.rs`.
- [x] 3.4 Actions in `agents_key`:
  - Enter opens the compose editor (only with a `target` and `can_receive`).
  - Ctrl+W watches; Ctrl+O sends `Action::Open(session_id)` or, on a peer row, shows the `cyber attach` toast.
  - Ctrl+R opens the inline rename (local and subagent rows only); Ctrl+G opens `/messages` with the held filter and the newest held message from that address.
  - Ctrl+L refreshes; Tab and Shift+Tab cycle the kind filter; typing, Backspace and Ctrl+U edit the filter.
  - Esc clears the filter, then closes.
  - A non-applicable action shows its reason.

  The compose editor sends on Enter, adds a newline on Shift+Enter or Alt+Enter, toggles `notify_when_idle` on Ctrl+N and cancels on Esc. It enforces the 16 KiB limit. A 409 shows the candidates and keeps the editor open.

## 4. Rendering and mouse

- [x] 4.1 `view/agents.rs` (new), built from `view::messaging::view`:
  - the title with the attached Session's name and the kind chips;
  - the filter line;
  - the group headers **This server**, **Peers** and **Project hub**, each with its loading or error line;
  - the columns state glyph, address, kind, state word, open/closed and Location, with Location dropped and then kind on narrow widths;
  - the compose and rename editors;
  - the empty and no-match states;
  - two hint lines.

  Use theme tokens only. Wire it in `view/mod.rs` (the full-screen match, the no-op arm and `full_screen`).
- [x] 4.2 `mouse.rs`: add `List::Directory`, the select and press-Enter arms, and `wheel_moves`.

## 5. Screen tests (`crates/cyber-tui/src/screens_agents.rs`, new)

- [x] 5.1 `agents_lists_local_sessions_and_subagents`, against a real server: a subagent and a second root, the attached Session absent. Covers "Local Sessions and a subagent". Snapshots `agents_dark_160x48`, `agents_light_160x48` and `agents_no_color_160x48`; the no-color one asserts that every cell is `Color::Reset` and the selected row is `REVERSED`.
- [x] 5.2 `agents_message_from_directory`: filter `migr`, Enter, type, Enter. The target Session's inbox has the message, and the toast says `delivered`. Covers "Message from the directory".
- [x] 5.3 `agents_watch_and_held_jump`: Ctrl+W creates a watch (asserted through the runtime's watch rows for the attached Session: one watch on `migration`), and Ctrl+G opens `/messages` with the held filter.
- [x] 5.4 `agents_rename_local_session`: Ctrl+R and a title. The row address becomes the title after the refresh. Covers "Rename a local Session".
- [x] 5.5 `agents_offline_peer_and_crafted_name`, using the `screens_peers.rs` two-machine setup with a dead remote. It checks the `*@nas` row, that Enter is refused, that a peer name with bidi and escape characters is cleaned, and that a send goes to the ID. Covers "Offline peer" and "Crafted peer name".
- [x] 5.6 `agents_hub_agent_row` (written; ignored on macOS, so not run in this change's local checks), Linux-only, like `screens_collaboration.rs`: a bound project with an online agent is listed as `hub agent`, Enter sends a direct hub message, and Ctrl+R gives its reason. Covers "Hub agent".
- [x] 5.7 `agents_messaging_disabled` and `agents_empty`: the disabled line, and the empty state with its hints.

## 6. Docs, spec and checks

- [x] 6.1 `docs/local-core-guide.md`: add `/agents` to the TUI messaging paragraph (near the `/messages` text) and the clickable panels list. `docs/features.md`: add `/agents` to the Cross-session messaging, Direct peers and Project collaboration rows. `docs/collaboration.md`: add a pointer from People to `/agents`. `ROADMAP.md`: change the "TUI agent directory" paragraph into a status line.
- [x] 6.2 Run `just specs` (OpenSpec strict validation, `spec_lint.py` with no new warnings, inventory). Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test -p cyber-tui -p cyber-server`, and diff any failures against `main`.
- [x] 6.3 (clippy's `cognitive_complexity` lint at the repository threshold of 15 passes; the cognitive-complexity skill has no Rust analyzer) Measure cognitive complexity of `agents.rs` and `view/agents.rs` against the frontend target (8–12). Split `agents_key` by mode (list, compose, rename) if it exceeds the target.
- [ ] 6.4 Accept by hand on the real binary with `cyber serve` and a scripted local provider: two Sessions, a subagent, a paired peer and an offline peer. Record the results in the PR body.
