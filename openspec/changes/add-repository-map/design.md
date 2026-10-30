## Context

The prior art is Aider's repository map, which we studied for behaviour only. It does three things:
- it extracts definition and reference tags with tree-sitter `tags.scm` queries;
- it builds a graph where file A links to file B when A references a name that B defines, and ranks it with PageRank, personalized by the files in the chat and the identifiers the user mentioned;
- it binary-searches how many ranked definitions fit a token budget, and prints file paths with the lines of interest.

Aider is Apache-2.0. We copy none of its code or queries.

Where we stand today:
- `crates/cyber-tools/Cargo.toml` already links `tree-sitter = "0.27"` with the Bash, Python, JavaScript and PowerShell grammars, for permission analysis of shell commands (`bash_analysis.rs`, `permissions/removals/`). No ctags use exists. The TUI uses `syntect` for highlighting only.
- `grep`/`glob`/`list` walk the tree with ripgrep's ignore rules (`builtin-tools` → glob, grep and list tools).
- The `lsp` tool offers `workspace_symbols` and `document_symbols`, but only with started servers. Its permission defaults to `ask` and it returns at most 50 rows (`code-intelligence` → lsp tool, Owned read-only navigation tool). Launching a language server requires a trusted checkout (Trusted local language-server launch).
- `system-context` renders Context Sources once per Context Epoch and reuses the baseline bytes for every Turn (Baseline reuse within an epoch). A changed source becomes one durable mid-conversation message at the next Safe Boundary (Reconciliation at Safe Boundaries). `provider-catalog` → Prompt caching relies on that byte-stable prefix.
- `improve-model-protocol-fidelity` adds a per-model behavior profile (`provider-catalog` → Model behavior profiles), resolved in layers. That change is implemented but not yet archived.

## Goals / Non-Goals

**Goals:**
- One cheap call, or zero calls, tells the model which files and definitions matter for the task.
- The prompt cache is never broken. The map in the baseline changes only at an Epoch boundary.
- The feature adds no runtime dependency. It works offline, in untrusted checkouts (tool only) and with any model.
- Output is deterministic, so tests can pin it byte for byte.

**Non-goals:** see proposal.md, including semantic search, LSP/ctags extraction, a watcher daemon and more grammars.

## Decisions

### D1. Symbol extraction: tree-sitter tag queries for a fixed language set

**Languages bundled:** Rust, Python, JavaScript (with JSX), TypeScript, TSX, Go, C, C++, Java, Solidity and Bash.
- Bash, Python and JavaScript are already linked.
- The new crates are pinned with `=` versions that are ABI-compatible with the `tree-sitter` 0.27 runtime.
- A CI step measures the stripped release binary against the main build and fails if the grammars add more than 12 MiB.

**Queries:**
- One `queries/<lang>.scm` per language, built into the binary.
- They start from each grammar's upstream `tags.scm` (MIT, notice kept), and we write our own where a grammar has none (Bash, Solidity).
- Captures follow the tree-sitter tags convention: `@name` plus `@definition.<kind>` or `@reference.<kind>`.
- The kinds we keep are `class`, `struct`, `enum`, `interface` (also traits), `type`, `function`, `method`, `constant`, `macro` and `module`. Fields, locals and parameters are dropped.

**References:**
- For a language whose query has no reference captures, every `identifier`-like leaf node whose text equals a defined name counts as a reference.
- Matching is by name only, so it is approximate. That is acceptable, because references only feed ranking.

**Signatures:**
- A signature is the source text from the start of the definition node to the start of its `body` field (or the end of its first line when the grammar has no `body` field).
- Whitespace runs are collapsed to one space, and the result is truncated to 160 characters with `…`.
- Comments and docstrings are not included. That keeps the map small and leaves less room for injected text.

**Other languages:** files are still listed by path and ranked by the references *into* them that we can see. They have no symbol rows.

*Alternatives rejected:*
- **LSP `documentSymbol`/`workspace/symbol`.** This needs an installed and running server per language. Indexing a large workspace takes minutes. There are no cheap reference counts for ranking. Starting servers needs a trusted checkout and puts process load on every Location. The results also depend on server versions, so they are not deterministic.
- **universal-ctags.** It is an external binary, so it cannot be part of the single static binary. Bundling it raises GPL-2 questions, and it gives no references.
- **Regex extractors.** They are cheap but wrong for nested and multi-line definitions in C++, TS and Rust, and every language becomes a bespoke parser.

### D2. Ranking: personalized PageRank over the file reference graph

- **Nodes and edges:**
  - The nodes are the indexed files.
  - For each name `x` that file A references and file B defines (A ≠ B), there is one edge A→B with weight `m(x) × sqrt(refs of x in A)`.
- **The multiplier `m(x)`** starts at 1.0, and:
  - is multiplied by 10 when `x` is in `focus_symbols`;
  - is multiplied by 0.1 when `x` starts with `_`, which marks a private name by convention;
  - is multiplied by 0.1 when `x` is defined in more than 5 files, since common names like `new`, `get` or `init` say little.
  
  Names shorter than 3 characters, and names defined in more than 200 files, are ignored.
- **Personalization:**
  - Each explicit `focus_paths` entry gets weight 1.0.
  - Each of the Session's 20 most recently read or edited distinct paths gets 0.5.
  - With none of these, the vector is uniform.
- **PageRank:**
  - The damping is 0.85.
  - It stops at 100 iterations or when the L1 change is below 1e-6.
  - Dangling mass is redistributed by the personalization vector.
  - Files are processed in path order, and sums use one thread in a fixed order, so `f64` results repeat across runs and platforms.
- **Definition rank:**
  - Each file's rank is split over its outgoing edges by weight, and definition (B, `x`) collects the shares of the edges into B for `x`.
  - A definition that nothing references gets `rank(B) × 0.001 / defs(B)`, so entry points still appear.
  - Ties are broken by path, then by line.

*Alternatives rejected:*
- Ranking by recency (mtime) or by file size: it says nothing about structure.
- LSP reference counts: too slow, see D1.
- Embeddings: see the semantic-search non-goal.
- Keeping chat files out of the map, as Aider does: our focus files are often not yet read, so they stay in, ranked first.

### D3. Rendering and budget

```
src/config.rs:
  12: pub struct Config {
  40: pub fn load(path: &Path) -> Result<Config>
src/server/mod.rs:
  88: impl Server {
  91:     pub async fn start(&self, cfg: Config) -> Result<()>
```

**Rendering:**
- Files are ordered by their best definition rank, and rows within a file by line.
- When a member is selected, its enclosing container line (`impl`, `class`) is printed too, indented by 4 per nesting level and capped at 2 levels. The container line costs tokens but does not count as a selection.
- After the ranked files, an `other files:` section lists files without selected rows, by file rank, while budget remains.

**Fitting the budget:**
- Tokens are estimated as `ceil(bytes / 4)`, the same estimate `tool-registry` uses.
- The renderer picks the largest prefix `k` of ranked definitions whose rendering fits the budget. Rendering only grows with `k`, so a binary search over `k` is exact and runs about 17 renders for 100 000 definitions.

**Budget sources:**
- The profile field `repo_map: { context_tokens, tool_tokens }` (D9) sets the defaults. All families start at 1 024 and 2 048.
- `context_tokens` is capped at 3% of the model's context window, rounded down to a multiple of 128. The source is omitted when the result is under 256, which happens for windows under about 8.5k tokens.
- `tool_tokens` is capped at 6% of the window, and `tokens` on a call is clamped to 256–8 192.

  For example, a 32k-window local model gets an 896-token baseline map and at most a 1 966-token tool map. A 200k-window model gets 1 024 and 2 048.
- A tool map is at most 8 192 × 4 = 32 KiB, which stays below `tool_output.max_bytes` (51 200), so it is never truncated by the registry.

### D4. Index storage: one local SQLite cache, content-addressed

**Location and schema:**
- Path: `<cache>/repo-map/index.db` (`~/.cache/cyber` by default). WAL, `busy_timeout` 5 s, file mode 0600. `PRAGMA user_version` holds the schema version.
- Tables:
  - `blobs(content_sha256, lang, extractor_version, tags BLOB)`. `extractor_version` hashes the query text and the grammar version, so a query change re-extracts.
  - `checkouts(root_id, canonical_root, last_used)`.
  - `files(root_id, path, size, mtime_ns, content_sha256, lang, excluded_reason)`.

**Worktrees:** worktrees of one repository have different roots but mostly the same blobs. A new worktree costs a walk plus hashing, with almost no parsing.

**Concurrency:**
- One build at a time per checkout and process. Concurrent callers join it, the way LSP spawns are deduplicated.
- Across processes, a lock file `<cache>/repo-map/<root_id>.lock` serializes builders. A caller that cannot get the lock reads the last committed snapshot and reports `building`.
- Writes are committed in batches of 500 files, so a crash loses at most one batch.

**Garbage collection:**
- After each build, blobs that no `files` row references are deleted.
- Checkouts unused for 30 days are dropped.
- Above 512 MiB, the least recently used checkouts are evicted.
- A schema version mismatch drops the database and rebuilds it.

*Alternatives rejected:*
- **The main session database.** It is durable, backed-up state behind a single-writer queue, while the index is a rebuildable cache that would bloat backups and compete with Session writes.
- **Files under `.cyber/` in the checkout.** They show up in `git status`, get committed by accident, and can be planted by a hostile repository.
- **In-memory only.** It would re-parse 50 000 files on every server start.
- **One file per checkout.** Worktrees would lose blob sharing.

### D5. File selection and invalidation (git-aware, no watcher)

**Walk:** the same walker as `grep`/`glob`. It honors `.gitignore`, `.ignore`, `.git/info/exclude` and the global git excludes, skips `.git/`, and does not descend into nested repositories or submodules.

**Default excludes:**
- the directories `node_modules/`, `vendor/`, `third_party/`, `target/`, `dist/`, `build/`, `.venv/` and `__pycache__/`;
- `*.min.js`, `*.min.css`, `*.map`, `*.lock`, `package-lock.json`, `*.pb.go`, `*_pb2.py` and `*.generated.*`;
- files whose first 5 lines contain `@generated` or `DO NOT EDIT`;
- paths marked `linguist-generated` or `linguist-vendored` in `.gitattributes`;
- files over 1 MiB, which is the LSP document budget;
- binaries, meaning a NUL byte in the first 8 KiB.

`repo_map.include` globs override the default excludes but not ignore files. `repo_map.exclude` adds to them. Every excluded file keeps its reason for `--status`.

**Refresh:**
- A refresh walks and stats. A file whose size or mtime changed is hashed, and when the hash changed it is extracted again. Removed files are deleted.
- `write`, `edit`, `apply_patch`, `notebook_edit` and patch-through-shell mark their paths dirty right away.
- A refresh runs before a `repo_map` call or a baseline render, when the last one is more than 10 s old or a dirty mark exists.
- The stat walk is bounded at 2 s. Past that, the last snapshot is used, marked `stale`, and the walk continues in the background.

*Alternatives rejected:*
- **A file watcher.** inotify limits, flaky network filesystems, and idle cost for every open Location.
- **`git ls-files` only.** It misses new untracked files, which are often the ones being worked on, and does not work outside git.

### D6. The map in the initial context: epoch-pinned Context Source

The model should see a map without having to ask, because weaker models rarely call an exploration tool before guessing. The cached prefix must not move, though. So:
- A Context Source can be registered `pinned`. It is observed when a baseline is rendered (Baseline initialization, Epoch replacement) and **skipped at Safe Boundary reconciliation**. Its stored snapshot value stays as it is, and it never emits an update or removal message.
- `core/repo-map` is pinned. Its value is rendered without personalization, from the latest committed index, at the context budget. Its header says it may be stale and points to `repo_map`.
- **Absent, never blocking.**
  - A pinned source that has not loaded within 2 s, or reports Unavailable Context, is omitted for that Epoch. It does not fail `ContextInitializationBlocked`.
  - The first Session in a fresh checkout often starts without a map. It gets one at its next Epoch, and the tool works right away.
- **Trust.** The source is rendered only in a trusted checkout, as defined by `workspace-trust`, and only for an agent whose `read` permission is not fully denied. Files whose `read` rules end in `deny` are left out.
- **Restarts.** The baseline already stores the rendered text, so a restart replays the same bytes.

*Alternatives rejected:*
- **A normal (reconciled) source.** Every edit would change the map, and each Safe Boundary would append a durable "map changed" message: token churn and transcript noise.
- **Tool only.** It loses the zero-call benefit for weak models, which is the main point.
- **Injecting the map into the first user message.** It mixes system data into the transcript, compaction would summarize it, and it shows up in exports as if the user wrote it.
- **Appending a fresh map after the cache breakpoint on each Turn.** It is uncached every Turn, costing about 1k tokens per Turn per Session.

### D7. A separate `repo_map` tool, not a new `lsp` operation

`lsp` checks the `lsp` permission (default `ask`), can start language servers and is offered only when code intelligence is enabled. The map has none of these properties. It is read-only, cheap and process-free, so it gets its own tool with the `grep` default rule and is allowed in plan mode. The registry rule is that a tool name must appear in the builtin-tools catalog, so the catalog requirement is modified to add `repo_map` (code-intelligence).

### D8. Inspection surfaces

- **CLI:** `cyber lsp map` sits in the existing `lsp` group owned by `code-intelligence`, so no top-level command is added.
  - It talks to the running server for the Location when one is registered. Otherwise it uses the cache in process, without starting a server or creating the session database.
  - `--format json` follows the CLI's single output flag.
- **API:** `GET /api/v1/lsp/map` and `POST /api/v1/lsp/map/rebuild` sit in the existing `lsp` route group, so no group is added to `server-api`.
- **TUI:** `/map` reuses the same API.
- **`/context`:** it already lists every source with its token estimate, and it now also marks pinned sources.

*Alternative rejected:* a top-level `cyber map` command. It needs a new entry in the cli-commands registry for one feature of an existing capability.

### D9. Budget per model profile

The profile from `improve-model-protocol-fidelity` gains `repo_map: { context_tokens, tool_tokens }`. It resolves through the same layers (family, prefix rules, catalog metadata, per-model config). The top-level `repo_map.context_tokens`/`tool_tokens` sits between built-in layers and per-model config: it beats layers 1–3, and loses to `providers.<p>.models.<m>.profile.repo_map`. The window caps from D3 always apply last.

Within an Epoch the pinned value does not change, even when the user switches to a model in the same family with a different budget. A provider-family switch already starts a new Epoch (Epoch replacement), so the new model gets its own budget then.

### D10. Large repositories, timeouts, scope

| Limit | Default | Config | When hit |
|---|---|---|---|
| Indexed files | 50 000 | `repo_map.max_files` | Files beyond the cap, in sorted walk order, are skipped; index `truncated` |
| File size | 1 MiB | `repo_map.max_file_bytes` | file excluded (`too_large`) |
| Parse time per file | 250 ms | none | file listed path-only (`parse_timeout`) |
| Full build wall time | 120 s | `repo_map.build_timeout_ms` | index `partial`; the next refresh continues |
| Stat walk before use | 2 s | none | last snapshot used, `stale` |
| Graph edges | 2 000 000 | none | the lowest-weight names are dropped first |
| Parser threads | `min(4, max(1, cpus / 2))` | none | n/a |

- **Automatic builds** start in the background when Location services start, but only when the Location is inside a git work tree and `repo_map.enabled` is true.
- **On-demand builds** cover any other directory, started by the first `repo_map` call or `cyber lsp map`.
- **Home or root as Location.** When the Location is the user's home directory or a filesystem root, nothing is indexed automatically, and a `repo_map` call without `path` fails with `RepoMapScopeError` asking for a subdirectory.
- **Tool wait.** A `repo_map` call waits at most 10 s for a running build and then answers from what is committed, with a `partial` or `building` header. A build in progress never makes the tool fail.

### D11. Privacy and trust

- **Local only.** The index holds paths, names and signature lines, which can be sensitive. It stays in the user's cache directory with mode 0600. It is never uploaded, shared, synced to runners, included in Session exports or backups, or sent to telemetry, which gets only counts, durations and status.
- **Runners.** A remote runner builds its own index on the runner.
- **Map text in transcripts.** Text that entered a transcript (a tool result, or the baseline) follows that transcript's existing export and redaction rules.
- **Untrusted checkouts.**
  - Indexing only parses files and never runs repository code, so it is safe and the tool works.
  - Only the baseline source waits for trust, because baseline text is in the privileged position.
  - The map is labeled as repository data (`workspace-trust` → Instruction and data provenance). Signature lines are capped and carry no comments. A literal `</repository_map>` in a signature is escaped.
  - If `add-untrusted-content-defense` defines a shared data envelope, the source adopts it. This change only requires the label.
- **Config needs no trust.** `repo_map` keys in project config are not security-sensitive: they cannot reach outside the Location, run anything or widen permissions.

### D12. Semantic search later, not now

See the proposal's non-goals. The index schema leaves room for it: a future `chunks` table keyed by `content_sha256` can be added without changing `blobs` or `files`.

## Risks / Trade-offs

- **Grammar ABI drift or abandoned grammars** (Solidity in particular). Mitigation: versions are pinned and CI parses a fixture per language. A grammar that breaks is dropped to path-only, which costs no build.
- **Binary growth.** Mitigation: the 12 MiB CI gate; the language set is the smallest that covers the primary languages, and SystemVerilog is excluded for its very large grammar.
- **A stale baseline map misleads the model.** Mitigation: the header states the map is a snapshot from the Epoch start, and the tool always refreshes. A long session gets a new map at compaction.
- **Name-only reference matching mis-ranks** in monorepos with repeated names. Mitigation: the >5-definitions damping, focus personalization and an eval (task 9.1) before the context source is on by default. If the eval shows no gain, `repo_map.context` defaults to `false` at release, and the decision is recorded in the proposal.
- **Prompt injection through identifiers.** Mitigation: no comments or docstrings, line caps, data labeling, and trusted checkouts only for the baseline.
- **Non-deterministic output** would make tests flaky and hurt caching across sibling subagents. Mitigation: a fixed order and single-threaded reductions, with a golden test that renders twice and on all CI platforms.
- **Shared registry conflicts.** Two MODIFIED registry requirements collide with other open changes. Mitigation: see Migration.

## Migration Plan

- **Existing work keeps its bytes.** No data migration. Existing Sessions keep their stored baseline bytes (Baseline reuse within an epoch), so `core/repo-map` first appears at their next Epoch. In-flight caches are untouched.
- **Rollback:** set `repo_map.enabled: false` or delete `<cache>/repo-map/`. Nothing else depends on the index.
- **Archive order:**
  - `improve-model-protocol-fidelity` must be archived first, because "Repository map budgets in model profiles" extends its "Model behavior profiles".
  - The MODIFIED "Capability-owned tool catalog" and "Top-level keys" deltas are written against `main` as of this change. If another change touching them archives first (for example `add-binary-hex-editor`, which adds `binary`), rebase this delta onto the new text: keep its lines and add `repo_map`. Do not drop theirs.
- **Feature flag:** none. `repo_map.enabled` and `repo_map.context` are the switches.

## Open Questions

- Should `focus_symbols` also be filled from identifiers in the latest user prompt, as Aider does? It is left out for now to keep the tool's output a function of its explicit inputs plus Session files. The eval can show whether it pays off.
- Should TOML/YAML/JSON keys be indexed for config-heavy repositories? Not in this change.
