## 1. Index schema and projector

- [ ] 1.1 Migration `<timestamp>_session_search.sql` in `crates/cyber-store/migrations/`:
  - the FTS5 table `session_search` (`porter unicode61 remove_diacritics 2`);
  - `session_search_doc` and its two indexes;
  - `session_search_state` (version, `indexed_bytes`, backfill cursor and target, seeded from `MAX(event.rowid)`);
  - `session_search_evicted`.

  Tests in `crates/cyber-store/src/migrate.rs`: `session_search_migration_creates_fts5_tables` (this also proves FTS5 is compiled into the bundled SQLite), and an empty database getting the full schema with the backfill row absent.
- [ ] 1.2 Text extraction in `crates/cyber-server/src/runtime/search_index.rs`: per-kind extraction and caps (D2), cuts on UTF-8 boundaries, tool summaries for `bash`/`powershell`/file tools/other tools, and tool output head and tail.
  - Unit tests: `extracts_each_kind_with_its_cap`, `cuts_on_char_boundary`, `tool_output_keeps_head_and_tail`, `tool_output_skipped_when_disabled`.
- [ ] 1.3 Redaction before insert, reusing the `observability-costs` secret patterns plus the high-entropy token rule.
  - Unit tests: `redacts_api_key_before_insert`, `redacts_high_entropy_token`.
  - Scenario test: "Secret in a prompt is redacted in the index".
- [ ] 1.4 Register the projector for `session.prompt.promoted.1`, `session.text.ended.1`, `session.tool.called.1`, `session.tool.settled.1`, `session.created.1`, `session.title.generated.1` and `session.renamed.1`. It must ignore `session.reasoning.ended.1` and the system, context and compaction events. Keep `indexed_bytes` current, and replace the title row in place.
  - Tests in `crates/cyber-app/tests/session_search.rs`:
    - `assistant_text_is_searchable_after_commit`;
    - `rolled_back_event_leaves_no_row`;
    - `opaque_reasoning_is_never_indexed`, which also asserts that no row contains the blob;
    - `held_prompt_is_not_indexed_until_promoted`;
    - `rename_replaces_title_row`.
- [ ] 1.5 `storage.search.enabled`, `storage.search.index_tool_output` and `storage.search.max_bytes` in the config schema, as global-only keys under `storage`.
  - Config tests: validation, defaults, and the minimum of 16 MiB.
  - Scenario test: "Search disabled".

## 2. Backfill, consistency and size bound

- [ ] 2.1 Resumable backfill:
  - batches of 2,000 events by rowid, using the projector code;
  - start 30 s after server start, 50 ms between batches, wait while the writer queue holds more than 64 transactions;
  - the `data_migration` row `session_search` when done;
  - `index.state` and `index.progress`.

  Tests: `backfill_resumes_after_restart_without_duplicates`, `backfill_waits_on_busy_writer_queue`, `search_during_backfill_reports_progress`.
- [ ] 2.2 Deletion: delete the doc and FTS rows inside `runtime/deletion.rs`'s transaction for every doomed Session, which covers retention GC too.
  - Regression tests: `deleted_session_disappears_from_search`, `child_sessions_rows_deleted_with_parent`, `retention_gc_removes_index_rows`, `delete_during_backfill_leaves_no_rows`.
- [ ] 2.3 Revert commit: delete the rows of the deleted messages, and mark hits off the active path after a conversation rewind as `active_path: false`.
  - Tests: `reverted_messages_leave_the_index`, `rewound_branch_hits_are_inactive`.
- [ ] 2.4 Index version check on open and after `cyber db restore`: on a mismatch, delete the rows and restart the backfill.
  - Tests: `restore_from_older_index_version_rebuilds`; `online_backup_carries_index`, which runs `cyber db backup`, opens the copy and searches it.
- [ ] 2.5 GC pass:
  - delete orphan rows, and run FTS5 `optimize` daily;
  - enforce `storage.search.max_bytes` by evicting the least recently updated Sessions to below 90% of the bound;
  - re-index an evicted Session on its next commit.

  Tests: `oldest_sessions_evicted_over_bound`, `evicted_session_reindexed_on_use`, `partial_flag_reported_while_evicted`, `orphan_fts_rows_never_surface`.

## 3. Query, ranking and snippets

- [ ] 3.1 Query parser (D5): words, phrases, an open quote that runs to the end, exclusions, `prefix*`, literal operators, quoting with `"` doubled, the 512-character and 16-term limits, and the rule that at least one term is not an exclusion.
  - Unit tests per form, including `fts_operators_are_literal`, `identifier_becomes_phrase` and `only_exclusions_refused`.
  - A `proptest` property: `no_query_text_causes_fts_syntax_error`, which runs arbitrary Unicode input against a real FTS5 table.
- [ ] 3.2 Scope resolution (`project` through project identity including worktrees, `directory`, `all`), the `session_id` filter, `archived=include|exclude`, and `parent_id`/`root_id` on results.
  - Tests: `default_scope_is_the_project`, `scope_all_includes_other_projects`, `archived_session_found_and_marked`.
- [ ] 3.3 Ranking `-bm25 / (1 + age_days / 30)`, a Session's score from its best hit, at most 3 hits per Session in transcript order, `sort=recent`, and cursor paging with limits 1 to 100.
  - Tests: `recent_session_outranks_equal_old_one`, `three_hits_per_session_in_order`, `cursor_pages_without_duplicates`.
- [ ] 3.4 Snippets: FTS5 `snippet()` with U+E000 and U+E001 markers, conversion to highlight ranges in scalar offsets, the 24-token and 300-character caps, and `…` at a cut.
  - Tests: `highlights_are_ranges_not_markup`, `content_containing_marker_chars_cannot_forge_highlight`.

## 4. Surfaces

- [ ] 4.1 Route `GET /api/v1/sessions/search` (`v1.session.search`) in `crates/cyber-server/src/http/sessions.rs`, Location-scoped, with the static segment matched before `:sessionID`. Return `400` and `503` errors as specified. Regenerate `sdk/openapi.json`.
  - HTTP tests: "Search the project" and "Static segment is not a Session ID".
  - The OpenAPI drift check.
- [ ] 4.2 SDK: run `just sdk-generate`. Add the `session.search` typed result and the cursor iterator helper.
  - SDK test: "Script finds a past Session" against an embedded server.
- [ ] 4.3 CLI `cyber sessions search` in `crates/cyber-cli`:
  - flags, table and JSON output, and bold highlights only on a TTY;
  - `No matching Sessions` and the backfill note on stderr;
  - exit 2 on an invalid query or `--format html`.

  CLI tests in `crates/cyber-cli/tests/cli.rs`: `sessions_search_lists_best_first`, `sessions_search_json_is_route_data`, `sessions_search_no_match_exits_zero`, `sessions_search_format_html_exits_2`, and `sessions_list_search_still_filters_titles`, a regression test.
- [ ] 4.4 TUI picker:
  - an **In transcripts** section after 3 characters and 250 ms, with up to 10 Sessions and highlighted snippets;
  - Tab to switch between scope `project` and `all`;
  - resume and scroll to the hit's message, highlighted until the next keypress;
  - the branch tree view for a hit off the active path;
  - `indexing <N>%` in the header, and hiding the section on errors.

  Tests in `crates/cyber-tui`, render and state tests against a real server: `picker_shows_transcript_matches`, `short_filter_sends_no_search`, `choosing_hit_scrolls_to_message`, `inactive_branch_hit_opens_tree`, `search_failure_keeps_title_matching`. Capture a PNG for the acceptance evidence.

## 5. Session search tool (P2)

- [ ] 5.1 The `session_search` tool in `crates/cyber-tools/src/tools/session.rs`:
  - search and fetch inputs, excluding the calling Session and its descendants;
  - the permission action `session_search` with the scope as resource, and no new default rule;
  - primary agents only, and `ForbiddenError` from child Sessions;
  - results wrapped as data with provenance `session_search` and the source Session ID, under the normal output limits.

  Tests in `crates/cyber-tools/tests/flow.rs`: `session_search_first_use_asks`, `subagent_cannot_search_other_sessions`, `session_search_excludes_calling_session`, `fetch_returns_redacted_indexed_text`.
  - Prompt-injection evaluation case: "Hostile text stays data", added to the `harness-evaluation` suite.

## 6. Performance and docs

- [ ] 6.1 Benchmark in `crates/cyber-store/benches/session_search.rs` with fixtures of 10,000 Sessions (manual) and 1,000 Sessions (CI). It measures query p95, projector p95 and backfill throughput.
  - The CI job gates the small fixture at 450 ms query p95 and 6 ms projector p95 (scenario "CI budget").
  - Record the full-size numbers in `docs/measurements/session-search.md`.
- [ ] 6.2 Docs:
  - `docs/local-core-guide.md`: search, scope, query syntax, what is and is not indexed, config keys;
  - `docs/features.md`;
  - `ROADMAP.md`: P1 table stakes, with the tool under P2.
- [ ] 6.3 At archive time, add `search` to the `sessions` list in `cli-commands` → Resource command groups (see design.md → Migration Plan). Then run `python3 scripts/spec_lint.py` and `openspec validate --strict`.
