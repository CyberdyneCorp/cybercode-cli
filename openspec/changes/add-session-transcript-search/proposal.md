## Why

A user who wants "the Session where we fixed the flaky login test" three weeks ago has no good way to find it. Today:

- `session-runtime` → Session listing and paging filters by title only (`GET /api/v1/sessions?search=`, a substring match on `session.title`). Generated titles are short and often generic, so the words the user remembers ("ECONNRESET", "parse_token", "flaky login") are almost never in them.
- The TUI Session picker (`tui` → Session picker and actions, `/resume`) fuzzy-matches names and titles only.
- `compaction` → Retrieval of earlier context gives the model `history_search`, but only inside the current Session, and it scans that Session's history in memory.
- `cyber db` can run SQL, but a `LIKE` over event JSON is slow, leaks opaque reasoning and secrets into the result, and is not something users should need.

Claude Code and Codex users ask for this regularly. The data is already local and durable; it only needs an index.

## What Changes

- **New capability `session-search`.** A local full-text index over Session transcripts, built with SQLite FTS5 (enabled in the bundled SQLite that `rusqlite`'s `bundled` feature builds; `libsqlite3-sys` 0.38.2 compiles with `SQLITE_ENABLE_FTS5`).
  - **Index projection.** The index is a projection maintained in the same transaction as the events that produce it, like the usage step ledger. It indexes user prompts, assistant text, tool names with an argument summary, the head and tail of tool output, attachment names and the Session title. It never indexes reasoning (text or opaque state), system and context messages, compaction summaries, attachment bytes, unpromoted inbox rows or text that the secret redaction rules match.
  - **Size caps.** Each indexed part is capped (32 KiB of prompt or assistant text, 512 bytes of tool-call summary, 2 KiB head plus 2 KiB tail of tool output). The whole index is bounded by `storage.search.max_bytes` (default 256 MiB of indexed text). Past it, the oldest Sessions are dropped from the index and searches report `partial`.
  - **Backfill.** Existing Sessions are indexed by a resumable background backfill in batches of 2,000 events. It yields to the writer queue, and searches during it say how far it has got.
  - **Consistency.** Deleting a Session (directly, by cascade or by retention GC) and committing a conversation revert remove its index rows in the same transaction. Backup carries the index; restore or a schema change rebuilds it.
  - **Query.** A small, safe query language: words, `"phrases"`, `-excluded` words and `prefix*`. User input is never passed to FTS5 `MATCH` unquoted, so it can never cause a syntax error or an injection.
  - **Ranking and results.** BM25 combined with recency, grouped by Session, with up to 3 hits per Session, each with a snippet and highlight ranges.
  - **Scope.** The current Location's project by default; one directory, or every Session in the database, on request.
- **Surfaces.**
  - **API** (`server-api`): `GET /api/v1/sessions/search` in the existing `session` group (operation `v1.session.search`).
  - **SDK** (`client-sdk`): generated `session.search`.
  - **CLI** (`cli-commands`): `cyber sessions search <query>`, a new subcommand of the existing `sessions` group. `cyber sessions list --search` stays a title filter.
  - **TUI** (`tui`): the `/resume` picker also shows transcript matches with snippets. Choosing one resumes that Session and scrolls to the matching message.
- **Model tool (P2).** `session_search` lets a primary agent search the user's other Sessions in the project and fetch one cited message. It is covered by the `session_search` permission action (default `ask` through the existing `* ask` rule). It is not offered to subagents, and its results are labeled untrusted data with provenance.

## Non-goals

- **Semantic or embedding search.** No vectors, no model calls at index or query time. A later change can add them on top of the same projection.
- **Cross-machine search.** Peers, Runners, the Relay and hosted storage are out of scope. The index lives in the local `cyber.db` only, and the tool never reaches another server.
- **Replacing `history_search`.** The current-Session tool keeps its contract. Moving it onto the index is a possible follow-up, not part of this change.
- **Duplicating memory.** Search returns transcript excerpts on request. It writes no memory entries and injects nothing into context on its own (`memory` stays the curated, always-loaded store).
- **Web dashboard search UI.** The route is there for it, and the dashboard can adopt it in a follow-up change.
- **Searching reasoning.** Reasoning summaries are not indexed either, not only opaque state (see design D2).

## Capabilities

### New Capabilities

- `session-search`:
  - "Transcript search index";
  - "Indexed transcript content";
  - "Search index backfill";
  - "Search index consistency";
  - "Search index size bound";
  - "Transcript search query syntax";
  - "Transcript search scope";
  - "Transcript search ranking and snippets";
  - "Transcript search performance";
  - "Session search tool" (P2).

### Modified Capabilities

All of the following are ADDED requirements. No existing requirement is modified, so this change cannot conflict with open changes that edit the same files.

- `server-api`: "Session search route".
- `client-sdk`: "Session search method".
- `cli-commands`: "Session search command".
- `tui`: "Session picker transcript matches".

## Impact

- **Code:**
  - `crates/cyber-store/migrations/`: one migration creating the FTS5 table `session_search`, the metadata table `session_search_doc`, the state table `session_search_state` and the backfill row.
  - `crates/cyber-server/src/runtime/`: a new `search_index.rs` projector, registered with the other projectors; calls from `deletion.rs` and the revert commit; backfill and GC hooks beside `usage_ledger.rs`.
  - `crates/cyber-server/src/http/sessions.rs`: the search route and its query type.
  - `crates/cyber-cli`: `cyber sessions search`.
  - `crates/cyber-tui`: picker section and jump-to-message.
  - `crates/cyber-tools/src/tools/session.rs`: the `session_search` tool (P2).
- **Data:** new tables only. No event schema changes. The index is derived, so it can always be dropped and rebuilt from events.
- **API, SDK and CLI:** a new route, a regenerated OpenAPI document (`sdk/openapi.json`) and SDK, and a new `sessions` subcommand. Config gains `storage.search.enabled`, `storage.search.index_tool_output` and `storage.search.max_bytes` under the existing `storage` key.
- **Privacy:** the index holds redacted copies of text already in the same database file. It is covered by the same file permissions, by SQLCipher when `storage.encrypt` is on, and by the in-memory database in ephemeral mode.
- **Docs:** `docs/local-core-guide.md` (search section), `docs/features.md` and `ROADMAP.md` (P1 table stakes). At archive time, `search` is added to the `sessions` list in `cli-commands` → Resource command groups (see design.md → Migration).
