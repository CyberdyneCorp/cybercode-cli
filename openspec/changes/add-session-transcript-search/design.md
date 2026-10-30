## Context

Where we stand today:
- Session data lives in `cyber.db`: SQLite in WAL mode, with `synchronous=FULL` and one bounded writer queue owned by the registered server (`docs/decisions/0001-storage-architecture.md`). The event log is authoritative. Projections (`session`, `tool_call`, `usage_step`, …) are written by projectors in the same transaction as their events (`storage-events` → Durable event store).
- `usage_ledger.rs` already shows how to add a projection to an existing database: a projector for new events, plus a resumable backfill over older events by `rowid` (`usage_backfill { cursor, target }`, 5,000 events per transaction, a `data_migration` row when it is done). Session deletion projects whatever the backfill has not yet reached, before it deletes the events (`usage_ledger::project_unreached`).
- `GET /api/v1/sessions?search=` filters on `session.title` (`ListQuery.search` in `crates/cyber-server/src/http/sessions.rs`). The TUI picker fuzzy-matches titles on the client.
- `history_search` (`crates/cyber-tools/src/tools/session.rs`) scans the current Session's history in memory.
- The bundled SQLite is built by `libsqlite3-sys` 0.38.2 with `-DSQLITE_ENABLE_FTS5` (`Cargo.toml`: `rusqlite = { version = "0.40", features = ["bundled", "backup"] }`). No new dependency is needed.

## Goals / Non-Goals

**Goals:**
- Find a Session by words from its transcript, weeks later, in well under a second.
- Never index what must not leave its place: opaque reasoning, secrets, attachment bytes.
- Keep the index exactly in step with deletion, revert and retention, without a second source of truth.
- Use one code path for live indexing and for backfill.

**Non-goals:** see proposal.md: semantic search, cross-machine search, replacing `history_search`, memory, dashboard UI.

## Decisions

### D1. An in-transaction projection, not an async indexer
The index is a projector. Like `usage_step`, it runs inside the event transaction for the event types listed in D2.

```
CREATE VIRTUAL TABLE session_search USING fts5(
  body, tokenize = 'porter unicode61 remove_diacritics 2');
CREATE TABLE session_search_doc (
  rowid INTEGER PRIMARY KEY,          -- same rowid as in session_search
  session_id TEXT NOT NULL, message_id TEXT, part_id TEXT,
  kind TEXT NOT NULL,                 -- title | user | assistant | tool_call | tool_output | attachment
  seq INTEGER NOT NULL, time INTEGER NOT NULL, bytes INTEGER NOT NULL) STRICT;
CREATE INDEX session_search_doc_session ON session_search_doc (session_id, seq);
CREATE INDEX session_search_doc_message ON session_search_doc (message_id);
CREATE TABLE session_search_state (
  id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL,
  indexed_bytes INTEGER NOT NULL, backfill_cursor INTEGER, backfill_target INTEGER) STRICT;
CREATE TABLE session_search_evicted (session_id TEXT PRIMARY KEY, time INTEGER NOT NULL) STRICT;
```

- FTS5 keeps its own copy of `body`, so `snippet()` and `highlight()` work. The metadata sits in an ordinary table that shares the rowid, so deleting a Session's rows is `DELETE … WHERE rowid IN (SELECT rowid FROM session_search_doc WHERE session_id = ?)`, an indexed lookup. An `UNINDEXED` FTS column would need a full scan.
- Cost: tokenizing at most 32 KiB per part inside the writer transaction. It is bounded by the caps in D2, and its target is 2 ms p95 per event (D9).

*Alternatives rejected:*
- **An async indexer** that reads committed events after a watermark. Indexing could never slow a commit, but it would add lag, a second consistency path for deletes and reverts (rows for a deleted Session indexed after its deletion), and its own crash recovery. The in-transaction projector inherits all of that from the event store.
- **An external-content FTS table over `event.data`.** The indexed text is not a column. It is extracted, capped and redacted JSON, so FTS5 could not rebuild or snippet from the source.
- **The trigram tokenizer.** It allows substring matches inside identifiers, but the index is about 3× larger, queries shorter than 3 characters match nothing, and stemming is lost. `unicode61` already splits `parse_token`, `src/auth.rs` and `ECONNRESET:` into tokens, and the query parser turns such input into a phrase (D5).
- **No stemming.** The user types "fix login bug" while the transcript says "fixed the login bugs". Porter stemming gives that recall. Both sides are stemmed, so identifiers still match each other.

### D2. What is indexed

| Source event | `kind` | Indexed text | Cap |
|---|---|---|---|
| `session.prompt.promoted.1` | `user` | text parts of the promoted user message (any `source`) | 32 KiB per part, first bytes kept |
| same | `attachment` | attachment `name` and file-mention paths, never bytes | 512 B |
| `session.text.ended.1` | `assistant` | the assistant text segment | 32 KiB |
| `session.tool.called.1` | `tool_call` | tool name, then a summary: `bash`/`powershell` the command, file tools their `path`/`pattern`, other tools compact JSON of the input | 512 B |
| `session.tool.settled.1` | `tool_output` | the model-visible output: first 2 KiB and last 2 KiB, when `storage.search.index_tool_output` (default `true`) | 4 KiB |
| `session.created.1`, `session.title.generated.1`, `session.renamed.1` | `title` | the current title (one row per Session, replaced) | 100 chars |

- **Never indexed:**
  - `session.reasoning.ended.1`: neither reasoning text nor opaque state (`improve-model-protocol-fidelity` → Reasoning items). Reasoning summaries are also left out: they are long, they repeat the assistant text, and under `improve-model-protocol-fidelity` they sit next to provider-signed state that must never be copied.
  - System and context messages (instructions, `CYBER.md`, context updates). They would match every Session of the project.
  - Compaction summaries. They repeat indexed text.
  - Side chats. Nothing of them is recorded.
  - Unpromoted, held or refused inbox rows.
  - Attachment bytes.
- **User shell commands** (`!cmd`) are indexed through their `bash` tool part, as `tool_call` and `tool_output`.
- **Tool output head and tail.** The head names the command. The tail carries the error a user remembers: compiler and test failures print it last.
- **Redaction before insert.** All text goes through `observability-costs` → Secret redaction before it is inserted, so a secret is replaced by `***` in the index and in every snippet. Searching for a key finds nothing, by design. Text that is still a single token of high entropy longer than 32 characters (the `memory` → Secret redaction rule) is replaced by `***` too.
- `storage.search.enabled: false` stops the projector and the backfill and drops the index tables' rows. Search then answers `503 ServiceUnavailableError` with `service: "session_search"`.

*Alternative rejected:* indexing full tool output. It would add most of the index size (a `cargo test` log can be megabytes) for little gain, because the full output is still available in the Session.

### D3. Backfill
The migration creates the tables and sets `backfill_target = MAX(event.rowid)` and `backfill_cursor = 0`, as `usage_backfill` does.

- A background task projects 2,000 events per writer transaction, in `rowid` order, with the same projector code. It waits 50 ms between batches, and waits longer while the writer queue holds more than 64 transactions, so prompts never wait on it. It starts 30 s after server start.
- The cursor is committed with each batch. A restart resumes from it.
- When the cursor reaches the target, the backfill clears the cursor and target and records a `data_migration` row named `session_search`.
- Events committed after the migration are indexed live, because their rowids are above the target.
- Deleting a Session while the backfill runs needs nothing special: the deletion removes the Session's doc rows and its events, so the backfill cannot reach them later.
- Search during the backfill returns what is indexed, with `index.state: "backfilling"` and `index.progress` (0–1).

### D4. Consistency with deletion, revert, retention and restore
- **Session deletion** (`runtime/deletion.rs`) deletes the doc and FTS rows of every doomed Session in its existing transaction, before the event rows go. Retention GC deletes archived Sessions through the same path, so it inherits this.
- **Revert commit** (`snapshots-checkpoints` → Three-phase revert) deletes the rows of the deleted messages in its own transaction. A conversation rewind that only moves the active branch keeps the rows. Such a hit is marked `active_path: false`.
- **Fork and import** write ordinary events, so their copies are indexed like any other. A fork's hits belong to the fork.
- **Backup** uses the SQLite online backup API, which copies the FTS shadow tables, so a backup carries the index.
- **Restore and upgrades.** On open, a database whose `session_search_state.version` differs from the binary's index version (tokenizer, caps or extraction rules changed) has its index rows dropped and the backfill restarted from `rowid` 0. The same happens on restore. This version is separate from migrations, so a rebuild never needs schema surgery.
- **Integrity.** Searches join `session_search_doc` and the `session` table, so an orphan FTS row can never surface. The daily GC pass removes orphans and runs FTS5 `optimize`.

### D5. Query syntax
User text is parsed by our own small grammar and never passed to `MATCH` as written:

- a bare word, which becomes a token; input that the tokenizer splits (`parse_token`, `a.b/c`) becomes a phrase of its tokens;
- `"a phrase"`; a quote left open runs to the end of the query;
- `-word` or `-"phrase"`, which excludes the term (at least one positive term is required);
- `word*`, a prefix match on a word of at least 2 characters.

Everything else is literal text, including `AND`, `OR`, `NEAR`, `:`, `(`, `)` and `^`. Each term is emitted as a quoted FTS5 string with `"` doubled, and the terms are joined with implicit AND. Limits: 512 characters and 16 terms. A query with no positive term, or that is too long, fails with `400 InvalidRequestError` and the reason. FTS5 syntax errors are therefore impossible by construction, and a property test checks that.

*Alternative rejected:* passing FTS5 syntax through behind a `--raw` flag. It gives power users little and re-opens the error and injection surface.

### D6. Scope, ranking and results
- **Scope:**
  - `project` (default): every Session whose directory belongs to the Location's project (`storage-events` → Project identity from git), including its worktrees.
  - `directory`: Sessions of the Location's directory only.
  - `all`: every Session in this database.
- **Other filters:**
  - Archived Sessions are included and marked (`archived: true`). `archived=exclude` leaves them out.
  - Child Sessions are included, with `parent_id` and `root_id`, so clients can group them under their root.
  - `session_id` restricts the search to one Session.
- **Score.** For each hit, `score = -bm25(session_search) / (1 + age_days / 30)`, where `age_days` is the age of the hit's message. A Session's score is its best hit's.
- **Ordering.** Sessions are ordered by score, and `sort=recent` orders by `updated_at` instead. Each Session returns up to 3 hits, in transcript order.
- **Hits.** A hit carries `{ message_id, part_id?, seq, kind, role, time, active_path, snippet: { text, highlights: [[start, end], …] } }`.
- **Snippets.** Each snippet comes from FTS5 `snippet()` with private-use marker characters U+E000 and U+E001 and at most 24 tokens. The server converts the markers to highlight ranges in Unicode scalar offsets and removes them, so content that contains the markers, or brackets, can never break or forge highlighting. A snippet is at most 300 characters, with `…` at a cut.
- **Paging.** Results are paged with an opaque cursor, limit 1–100 (default 20), as in `server-api` → Pagination.

*Alternative rejected:* returning one row per hit rather than grouping by Session. The user is looking for a Session, and one Session that discusses the topic at length would fill the page.

### D7. Surfaces
- **API.** `GET /api/v1/sessions/search?q=&scope=&archived=&session_id=&sort=&limit=&cursor=`, in the `session` group. It is Location-scoped and its response is wrapped as `{ location, data }`. `data = { sessions: [{ id, title, directory, parent_id, root_id, archived, updated_at, score, hits }], index: { state, progress?, partial, evicted_sessions }, cursor? }`. The static `search` segment cannot collide with Session IDs, which all start with `ses_`.
- **CLI.** `cyber sessions search <query> [--scope project|directory|all] [--all] [--exclude-archived] [--sort relevance|recent] [--limit N] [--format table|json]`. It goes through the registered server, starting it when needed, like the other `sessions` subcommands.
  - `--all` means `--scope all`.
  - The table prints one Session per row (ID, title, relative time) and its best snippet indented underneath, with highlights in bold on a TTY and without markup otherwise.
  - No match prints `No matching Sessions` to stderr and exits 0.
  - A note `index incomplete: backfilling 42%` goes to stderr while the backfill runs.
  - `--format json` prints `data` unchanged.
  - The title-only filter `cyber sessions list --search` keeps its meaning.
- **TUI.** In `/resume`, title matches stay as they are. When the filter text has at least 3 characters and has not changed for 250 ms, the picker also calls the route with scope `project` and shows a **In transcripts** section with up to 10 Sessions, each with its best snippet highlighted. Enter on a hit resumes that Session and scrolls the transcript to the hit's message, which is highlighted until the next keypress. A hit off the active path opens the branch tree view (`tui` → Branch tree view) at that message instead. Tab inside the picker toggles the scope between `project` and `all`.
- **SDK.** The generated `session.search(query, options)`.

### D8. The `session_search` tool (P2)
It is P2 because it is agent-facing reach across Sessions, which belongs with the P2 orchestration work. Users get value from P1 search without it.

- **Input:** `{ query, scope?: "project" | "all", limit? (default 5, max 20) }`, or `{ session_id, message_id }` to fetch one message's indexed text (at most 4 KiB, as stored, so redacted).
- **Exclusions.** It searches the Sessions of the scope minus the calling Session and its descendants, which `history_search` already covers.
- **Visibility.** It is registered for primary agents only. Subagent-only and hidden agents are never offered it, and a call from a child Session fails with `ForbiddenError`. This keeps `compaction` → Retrieval of earlier context's rule that child Sessions gain no access to unrelated Sessions.
- **Permission.** The permission action is `session_search`, with the scope as resource (`session_search:project`, `session_search:all`). No new default rule is added: the existing `* ask` applies, so the first use asks, and the user can save `allow`.
- **Provenance.** Results are wrapped as data with provenance `session_search` and the source Session ID (`workspace-trust` → Instruction and data provenance). They are never instructions, cannot approve anything and do not survive compaction as authority.
- **Ephemeral databases.** On an in-memory (ephemeral) database the tool sees only that process's Sessions, like everything else.

*Alternatives rejected:*
- **Default `allow`, as for `list_sessions`.** `list_sessions` returns names and states, while this tool returns transcript content from other projects with `all`.
- **P1.** The user-facing search does not depend on it.

### D9. Performance targets and size bound
- **Query.** p95 ≤ 150 ms for a 3-term query with 10,000 Sessions, 1,000,000 indexed rows and 256 MiB of indexed text, on a laptop SSD with a warm cache.
- **Projector.** ≤ 2 ms p95 added per indexed event at the 32 KiB cap.
- **Backfill.** At least 20,000 events per second when idle.
- These targets are measured by a benchmark in `crates/cyber-store/benches`. They are not gated in CI. CI gates a 10× smaller fixture at 3× the budget.
- **Size bound.** `storage.search.max_bytes` (default 256 MiB, minimum 16 MiB, `0` for no bound) limits the sum of indexed text bytes, which is tracked in `session_search_state.indexed_bytes`. The on-disk index is expected to be at most 3× that.
  - When a GC pass finds the sum over the bound, it drops the rows of the Sessions with the oldest `updated_at` until the sum is below 90% of the bound, and records them in `session_search_evicted`.
  - An evicted Session that gets new events is indexed again from its events, inside that Session's next commit.
  - Searches report `index.partial: true` and `evicted_sessions` while any Session in scope is evicted.

*Alternative rejected:* refusing to index past the bound. Then new Sessions, which are the most likely to be searched, would be the ones missing.

## Risks / Trade-offs

- **Write amplification on hot Sessions.** A tool-heavy Drain commits many small events, and each one indexed adds FTS5 segment writes. Mitigation: the caps; automerge, which FTS5 does by default; the daily `optimize`; and the projector benchmark. If the target is missed, tool output indexing can be turned off (`storage.search.index_tool_output`).
- **Database growth.** Even with the bound, a large first backfill grows `cyber.db` and its WAL. Mitigation: batches commit separately, so WAL checkpoints keep up. `cyber doctor` (follow-up) can report the index size from `session_search_state`.
- **Redaction is pattern-based.** A secret that the patterns miss is indexed. This is no new exposure, since the same text is already in the events of the same file, but snippets make it easier to see. The redaction rules are shared with telemetry, so a fix there covers both.
- **Stemming surprises.** Porter can over-match (for example `university`/`universe`). Phrases still match exactly by position, and bm25 ranks exact forms first in practice.
- **Index version churn.** Changing a cap or an extraction rule forces a full rebuild. Mitigation: the rebuild is the backfill, which is background and resumable.
- **Prompt injection through the tool.** A past transcript can contain hostile text. Mitigation: the provenance labeling and the `ask` default (D8); the tool is P2.

## Migration Plan

1. **Migration.** One migration, `<timestamp>_session_search.sql`, creates the four tables and the backfill state. Older binaries open a database that has it as they open any newer database (`DatabaseTooNewError`, read-only).
2. **First start.** The first start after upgrade begins the backfill. The search route answers at once with partial results.
3. **Downgrade.** None is needed beyond the existing rule. The tables are derived and can be dropped: `cyber db vacuum` with `storage.search.enabled: false` reclaims the space.
4. **Archive.** When this change is archived, `search` is added to the `sessions` subcommand list in `cli-commands` → Resource command groups. That list is "at least these subcommands", so the ADDED requirement is valid on its own, and leaving the edit to archive time avoids a conflict with the open changes that already modify that requirement (`add-session-rename`, `add-messaging-direct-peers`).

## Open Questions

- Should `history_search` move onto the index? It would gain ranking and speed, but it also searches compacted originals beyond the caps. That is left to a follow-up.
- Should the web dashboard's Session list adopt the route in the same release? Not required here.
