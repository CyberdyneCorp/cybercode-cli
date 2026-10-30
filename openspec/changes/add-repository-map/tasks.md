## 0. Preconditions

- [ ] 0.1 Archive `improve-model-protocol-fidelity` first, since "Repository map budgets in model profiles" extends its "Model behavior profiles". Then rebase the MODIFIED "Capability-owned tool catalog" and "Top-level keys" deltas onto the current `main` text, keeping any lines other archived changes added.
  - Check: `openspec validate add-repository-map --strict` and `python3 scripts/spec_lint.py` with no new warnings.

## 1. Grammars and extraction

- [ ] 1.1 Add pinned grammar crates for Rust, TypeScript/TSX, Go, C, C++, Java and Solidity to `crates/cyber-tools/Cargo.toml`. They must be ABI-compatible with `tree-sitter` 0.27.
  - Add a CI step that compares the stripped release binary with `main` and fails above +12 MiB.
  - Add the MIT notices to the third-party notices file.
  - Test: `grammars_load_for_every_bundled_language` parses one fixture per language.
- [ ] 1.2 Add `repo_map/queries/<lang>.scm`, starting from each grammar's upstream `tags.scm` and writing our own for Bash and Solidity, and `repo_map/extract.rs`:
  - kinds and container;
  - signature up to the `body` field, collapsed and capped at 160 characters;
  - no comments or docstrings;
  - identifier fallback for references;
  - the 250 ms parse timeout.
  
  Tests:
  - golden tag fixtures per language;
  - "Rust signature without the body";
  - `docstrings_are_not_in_signatures`;
  - `parse_timeout_lists_file_by_path` (a pathological fixture);
  - "Unsupported language listed by path".

## 2. File selection and storage

- [ ] 2.1 `repo_map/walk.rs`:
  - reuse the `grep`/`glob` ignore walker and skip submodules and nested repositories;
  - the default excludes, `@generated`/`DO NOT EDIT`, `.gitattributes` linguist attributes, size and binary checks;
  - `include`/`exclude` globs, refusing globs outside the Location;
  - an exclusion reason for every skipped file.
  
  Tests: "Generated protobuf skipped", "Include overrides a default exclude", `symlink_outside_location_not_followed`, `linguist_vendored_excluded`.
- [ ] 2.2 `repo_map/store.rs`:
  - `<cache>/repo-map/index.db` with WAL, a 5 s busy timeout and mode 0600;
  - the `blobs`/`checkouts`/`files` schema with `user_version`;
  - stat-then-hash change detection and 500-file commit batches;
  - garbage collection: unreferenced blobs, 30 days unused, the 512 MiB cap;
  - drop and rebuild on a schema version mismatch.
  
  Tests: "Second worktree reuses tags" (counts parser invocations), "Deleted cache rebuilt", `schema_mismatch_rebuilds`, `gc_evicts_least_recently_used_over_cap`, `index_file_mode_is_0600` (Unix).
- [ ] 2.3 Build coordination:
  - one build at a time per checkout, with joined callers;
  - the cross-process `<root_id>.lock`, and readers getting committed state while locked;
  - the `max_files`/`build_timeout_ms` limits with the `truncated`/`partial` status;
  - continuation on the next refresh;
  - `min(4, max(1, cpus/2))` threads.
  
  Tests: "Huge monorepo capped" (a generated 60 000-file tree with `max_files` lowered for speed), `two_processes_one_builder`, `partial_build_resumes`.

## 3. Ranking and rendering

- [ ] 3.1 `repo_map/graph.rs` and `rank.rs`:
  - edges and the `m(x)` multipliers;
  - the ignore rules for short names and names defined in more than 200 files;
  - the 2 000 000-edge cap;
  - personalization from focus paths and Session files;
  - PageRank with damping 0.85, 100 iterations and a 1e-6 tolerance;
  - definition rank with the unreferenced floor;
  - deterministic order.
  
  Tests: "Focus symbol pulls in its definition", `private_and_common_names_damped`, `unreferenced_entry_point_still_ranked`, and "Same input, same output", which runs in CI on Linux, macOS and Windows against one golden file.
- [ ] 3.2 `repo_map/render.rs`:
  - the path/row format and container lines;
  - `other files:`;
  - `ceil(bytes/4)` estimation and the binary search for the largest prefix that fits.
  
  Tests: "Budget respected", "Container line shown", `render_is_monotonic_in_prefix_length` (a property test).

## 4. Freshness and Location services

- [ ] 4.1 `cyber-server`:
  - start the background build when Location services start, only inside a git work tree;
  - no automatic build in the home directory or a filesystem root;
  - refresh-on-use with the 10 s debounce;
  - the 2 s stat bound with the `stale` status;
  - dirty marks from `write`, `edit`, `apply_patch`, `notebook_edit` and patches through the shell.
  
  Tests: "Own edit visible immediately", "Home directory refused", `stale_when_stat_walk_exceeds_bound` (an injected slow filesystem), `non_git_location_builds_on_demand`.

## 5. Tool and permissions

- [ ] 5.1 Write `tools/repo_map.rs` with the schema, argument clamping and the header line, and with `not indexed:` notes and `No indexed files`. The tool waits at most 10 s for a build, uses the `repo_map` permission with the `grep` default rule, is offered in plan mode, and filters out files whose `read` rule ends in deny. Register it in the capability-owned catalog.
  - Tests: "Focused map for a weak model", "Denied file hidden", "First call during the initial build", `tool_hidden_when_repo_map_disabled`, `offered_in_plan_mode`.
  - Update the tool schema bundle (`GET /api/v1/tools/schema`) snapshot.
- [ ] 5.2 Add the Session-file personalization: the 20 most recently read or edited distinct paths, taken from settled tool calls.
  - Test: `recent_session_files_bias_ranking`.

## 6. Context Source and profiles

- [ ] 6.1 `cyber-core` system context:
  - the `pinned` flag on source registration;
  - reconciliation skips pinned sources;
  - the 2 s load bound and omission without `ContextInitializationBlocked`;
  - the `pinned` marker in `/context` and the context API.
  
  Tests: "Pinned source changes mid-epoch" (asserts the byte-identical serialized prefix of two requests), "Slow pinned source at the first Turn", and `restart_replays_pinned_bytes`.
- [ ] 6.2 Add `core/repo-map`:
  - the `<repository_map>` block with its data header and `</repository_map>` escaping;
  - omission rules (disabled, untrusted checkout, `read` fully denied, budget under 256, empty index) and the per-agent `read` filtering.
  
  Tests: "Map in the baseline", "Edits do not move the prefix" (compaction then a fresh map), "Untrusted checkout", `closing_tag_in_signature_escaped`.
- [ ] 6.3 `catalog/profile.rs`:
  - the `repo_map` field with family defaults;
  - config precedence (top-level over layers 1–3, per-model config over top-level);
  - window caps applied last;
  - the field in `cyber models --json`.
  
  Tests: "Small local model", "Per-model override wins over the top-level key".
- [ ] 6.4 Config schema for `repo_map` in `cyber-core`, with invalid-value warnings. Regenerate the JSON schema.
  - Tests: "Map turned off for a project", "Invalid budget ignored".

## 7. CLI, API and TUI

- [ ] 7.1 Add `GET /api/v1/lsp/map` (`v1.lsp.map`) and `POST /api/v1/lsp/map/rebuild` (`v1.lsp.mapRebuild`) with the Location envelope. Regenerate `sdk/openapi.json` and the TS SDK.
  - Tests: route tests for both, an OpenAPI snapshot, and an SDK type-check.
- [ ] 7.2 Add `cyber lsp map` with `[<path>]`, `--focus`, `--symbol`, `--tokens`, `--format text|json`, `--status`, `--rebuild` and `--clear`. It uses the server when one is registered and runs in process otherwise.
  - Tests: "Inspect without a server" (asserts no server process and no session database), "JSON for scripts", `clear_removes_checkout_rows`.
  - Update `docs/cli.md`.
- [ ] 7.3 TUI `/map [<path>]`: the status header, `r` rebuild, `Enter` to open the file explorer, `Esc` to close, and the disabled message.
  - Screen tests: "Inspect the map", "Map disabled".

## 8. Privacy

- [ ] 8.1 Keep paths, names and signatures out of logs and OTel. The span `repo_map.build` carries counts, durations and status only. The index is not part of exports, shares, backups or runner sync.
  - Tests: "Telemetry without code" (an in-memory exporter), `export_excludes_index`, "Untrusted checkout still mapped by the tool" (asserts no hook, formatter or LSP spawn).

## 9. Evidence

- [ ] 9.1 Run the pinned-repository eval (`harness-evaluation`) with and without the map (`repo_map.enabled: false`) on one hosted model and one local model (Ollama, a 32k window). Record pass rate, exploration tool calls before the first edit, and input tokens in `docs/implementation/`.
  - If the local model shows no gain, set the `repo_map.context` default to `false` and record that in proposal.md.
- [ ] 9.2 On the same runs, record how often the agent still needed `grep`/`lsp` after reading the map. This is the input for whether `add-semantic-code-search` is worth proposing.
- [ ] 9.3 Benchmark on a 50 000-file repository: cold build time, warm refresh time, cache size, and peak memory. Record the results next to the eval.

## 10. Docs and checks

- [ ] 10.1 Docs:
  - `docs/local-core-guide.md`: the repository map, epoch-pinned sources, `core/repo-map` and the limits table;
  - a row in `docs/features.md`;
  - the ROADMAP entry.
- [ ] 10.2 Run the gate:
  - `cargo fmt`;
  - workspace clippy with `-D warnings`;
  - `cargo nextest run --workspace --profile ci`;
  - OpenAPI/SDK/CLI doc checks;
  - `openspec validate --all --strict`;
  - `python3 scripts/spec_lint.py`, with no new warnings;
  - the binary-size gate from 1.1.
  
  Keep cognitive complexity ≤ 15 per function. The graph and the renderer must stay within that limit.
