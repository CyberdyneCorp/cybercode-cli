## ADDED Requirements

### Requirement: Repository map configuration
(P1) The system SHALL read repository map settings from the top-level `repo_map` key with these fields and defaults:
- `enabled` (default `true`): when `false`, no index is built, `repo_map` is not advertised and `core/repo-map` is omitted;
- `context` (default `true`): whether `core/repo-map` is rendered into baselines;
- `context_tokens` and `tool_tokens` (default unset): budgets that override the built-in profile layers for every model;
- `include` and `exclude` (glob arrays, default empty): `include` overrides the default excludes but not ignore files, and `exclude` adds to them;
- `max_files` (default 50000), `max_file_bytes` (default 1048576) and `build_timeout_ms` (default 120000).

An invalid value SHALL be ignored with a warning naming the key, and the default SHALL apply. These keys SHALL NOT be part of the checkout's trust digest, and a glob that resolves outside the Location SHALL be ignored with a warning.

#### Scenario: Map turned off for a project
- **WHEN** the project `cyber.jsonc` sets `repo_map.enabled = false`
- **THEN** no index is built for that Location, `repo_map` is not in the advertised tools and `/context` lists no `core/repo-map`

#### Scenario: Invalid budget ignored
- **WHEN** config sets `repo_map.tool_tokens = "big"`
- **THEN** a warning names `repo_map.tool_tokens` and the profile budget is used

### Requirement: Repository map file selection
(P1) The repository index SHALL select files using the same ignore-aware walk as `grep` and `glob`:
- it SHALL honor `.gitignore`, `.ignore`, `.git/info/exclude` and the global git excludes;
- it SHALL skip `.git/`, and SHALL NOT descend into nested repositories or submodules.

It SHALL also exclude by default:
- the directories `node_modules/`, `vendor/`, `third_party/`, `target/`, `dist/`, `build/`, `.venv/` and `__pycache__/`;
- the patterns `*.min.js`, `*.min.css`, `*.map`, `*.lock`, `package-lock.json`, `*.pb.go`, `*_pb2.py` and `*.generated.*`;
- files whose first 5 lines contain `@generated` or `DO NOT EDIT`;
- paths with `linguist-generated` or `linguist-vendored` set in `.gitattributes`;
- files larger than `repo_map.max_file_bytes`;
- files with a NUL byte in their first 8 KiB.

Each excluded file SHALL keep one reason: `ignored`, `default_exclude`, `generated`, `vendored`, `too_large`, `binary` or `config_exclude`. Symlinks whose canonical target is outside the Location SHALL NOT be followed.

#### Scenario: Generated protobuf skipped
- **WHEN** the Location contains `api/user.pb.go` and `api/user.go`
- **THEN** only `api/user.go` is indexed, and `cyber lsp map --status` counts `api/user.pb.go` under `default_exclude`

#### Scenario: Include overrides a default exclude
- **WHEN** config sets `repo_map.include = ["vendor/acme/**"]` and `vendor/` is not in `.gitignore`
- **THEN** files under `vendor/acme/` are indexed and other `vendor/` files stay excluded

### Requirement: Repository map symbol extraction
(P1) The system SHALL extract definitions and references with tree-sitter tag queries built into the binary for Rust, Python, JavaScript (with JSX), TypeScript, TSX, Go, C, C++, Java, Solidity and Bash.

**Definitions:**
- Each definition SHALL record its name, its kind (`class`, `struct`, `enum`, `interface`, `type`, `function`, `method`, `constant`, `macro` or `module`), its 1-based line and its enclosing container, if any.
- Fields, locals and parameters SHALL NOT be recorded.

**Signatures:**
- A definition's signature SHALL be the source text from the start of the definition node to the start of its `body` field, or the end of its first line when there is no `body` field.
- Whitespace runs SHALL be collapsed to one space, and the result truncated to 160 characters ending in `…`.
- Comments and docstrings SHALL NOT be part of a signature.

**References:**
- For a language whose query has no reference captures, identifier leaf nodes whose text equals a defined name SHALL count as references.

**Files without symbols:**
- Files in other languages SHALL be indexed by path only, with no symbols.
- A file whose parse exceeds 250 ms SHALL also be indexed by path only, with reason `parse_timeout`.
- Extraction SHALL NOT start a language server or run any repository code.

#### Scenario: Rust signature without the body
- **WHEN** `src/config.rs` line 40 defines `pub fn load(path: &Path) -> Result<Config> {` with a 30-line body
- **THEN** the index holds a `function` named `load` at line 40 with signature `pub fn load(path: &Path) -> Result<Config>`

#### Scenario: Unsupported language listed by path
- **WHEN** the Location contains `rtl/core.sv`
- **THEN** the index lists `rtl/core.sv` with no symbols, and ranking can still place it through references to it from indexed files

### Requirement: Repository map index storage
(P1) The system SHALL keep the index in a local SQLite cache at `<cache>/repo-map/index.db`, created with file mode 0600, using WAL and a 5 s busy timeout. The database SHALL hold three things:
- extracted tags keyed by the file's SHA-256 content hash and an extractor version, derived from the query text and grammar version;
- one row per checkout, keyed by canonical checkout root;
- one row per indexed file, with path, size, mtime, content hash, language and exclusion reason.

**Change detection:**
- A file whose size or mtime changed SHALL be hashed again. Its tags SHALL be extracted again only when the hash or extractor version has no stored tags.

**Concurrency:**
- Builds for one checkout SHALL be serialized: callers in one process join the running build, and processes coordinate through `<cache>/repo-map/<root_id>.lock`.
- A caller that cannot get the lock SHALL read the last committed state.
- Results SHALL be committed in batches of at most 500 files.

**Garbage collection:**
- After a build, tags no file row references SHALL be deleted.
- Checkouts unused for 30 days SHALL be dropped.
- Above 512 MiB, the least recently used checkouts SHALL be evicted.
- A schema version mismatch SHALL drop and rebuild the database.

The index SHALL NOT be written inside the checkout or into the session store.

#### Scenario: Second worktree reuses tags
- **WHEN** a repository is indexed and the user creates a worktree of it with `cyber worktree`, then calls `repo_map` there
- **THEN** the new checkout gets its own file rows, and files with unchanged content are not parsed again

#### Scenario: Deleted cache rebuilt
- **WHEN** the user deletes `<cache>/repo-map/` while the server runs
- **THEN** the next `repo_map` call rebuilds the index and answers without error

### Requirement: Repository map build limits and freshness
(P1) The system SHALL build and refresh the repository index within the bounds below.

**Automatic and on-demand builds:**
- The system SHALL start an index build in the background when Location services start, if `repo_map.enabled` is true and the Location is inside a git work tree.
- Any other Location SHALL be indexed on demand by the first `repo_map` call or `cyber lsp map`.
- Builds SHALL use at most `min(4, max(1, cpus / 2))` parser threads.

**Limits:**
- A build SHALL index at most `repo_map.max_files` files in sorted walk order and mark the index `truncated` when files remain.
- It SHALL stop after `repo_map.build_timeout_ms` and mark the index `partial`. The next refresh SHALL continue from what was committed.
- When the Location is the user's home directory or a filesystem root, no build SHALL start automatically, and a `repo_map` call without `path` SHALL fail with `RepoMapScopeError` asking for a subdirectory.

**Refresh:**
- A refresh SHALL run before each `repo_map` call and each `core/repo-map` render when the last refresh is older than 10 s or a path is marked dirty.
- `write`, `edit`, `apply_patch`, `notebook_edit` and patches applied through the shell SHALL mark their paths dirty when they settle.
- A refresh whose stat walk exceeds 2 s SHALL use the last committed state marked `stale` and finish in the background.

#### Scenario: Huge monorepo capped
- **WHEN** a Location has 180 000 candidate files and the default `max_files`
- **THEN** the first 50 000 files in sorted walk order are indexed, and the `repo_map` header reports `truncated`

#### Scenario: Own edit visible immediately
- **WHEN** the agent adds `pub fn retry_policy()` to `src/net.rs` with `edit` and then calls `repo_map` with `focus_paths: ["src/net.rs"]` 3 s later
- **THEN** the map lists `retry_policy` under `src/net.rs`

#### Scenario: Home directory refused
- **WHEN** `cyber` is started in the user's home directory and the model calls `repo_map` with no `path`
- **THEN** the call fails with `RepoMapScopeError` and no index build starts

### Requirement: Repository map ranking
(P1) The system SHALL rank files with personalized PageRank over a graph of indexed files.

**Edges:**
- For each name `x` that file A references and file B defines, with A different from B, there SHALL be one edge from A to B with weight `m(x) × sqrt(number of references to x in A)`.
- `m(x)` SHALL be 1.0, multiplied by 10 when `x` is a focus symbol, by 0.1 when `x` starts with `_` and by 0.1 when `x` is defined in more than 5 files.
- Names shorter than 3 characters, and names defined in more than 200 files, SHALL be ignored.
- At most 2 000 000 edges SHALL be kept, dropping the lowest-weight names first.

**Personalization:**
- Weight 1.0 for each focus path, and 0.5 for each of the Session's 20 most recently read or edited distinct paths.
- A uniform vector when there are neither.

**PageRank:**
- Damping SHALL be 0.85, with at most 100 iterations, stopping when the L1 change is below 1e-6.
- Dangling rank SHALL be redistributed by the personalization vector.

**Definition rank:**
- Each file's rank SHALL be split over its outgoing edges by weight, and each definition SHALL receive the shares of the edges for its name into its file.
- A definition that nothing references SHALL receive `file rank × 0.001 / definitions in the file`.

**Determinism:** ties SHALL be broken by path, then by line. Computation SHALL process files in path order with single-threaded reductions, so equal inputs produce identical ranks on every platform.

#### Scenario: Focus symbol pulls in its definition
- **WHEN** `repo_map` is called with `focus_symbols: ["RetryPolicy"]` and `RetryPolicy` is defined in `src/net/retry.rs`
- **THEN** `src/net/retry.rs` and the files that reference `RetryPolicy` rank above files that do not

#### Scenario: Same input, same output
- **WHEN** the same index and the same arguments are ranked twice, on Linux and on Windows
- **THEN** both renderings are byte-identical

### Requirement: Repository map rendering and token budget
(P1) The system SHALL render every repository map in the format below and fit it to a token budget.

**Format:**
- The system SHALL render a map as one `<path>:` line per file, followed by one `  <line>: <signature>` row per selected definition in line order.
- Files SHALL be ordered by their best definition rank.
- A selected member SHALL be preceded by its enclosing container's row when that row is not itself selected, indented 4 spaces per nesting level, up to 2 levels.
- Files without selected rows SHALL follow under `other files:`, one path per line, in file rank order, while budget remains.
- Paths SHALL be Location-relative with forward slashes.

**Budget:**
- Tokens SHALL be estimated as `ceil(bytes / 4)`.
- The renderer SHALL select the largest prefix of ranked definitions whose full rendering fits the budget, then add other-file paths while they fit.

#### Scenario: Budget respected
- **WHEN** a map is rendered with a 1024-token budget for a repository with 30 000 definitions
- **THEN** the rendering is at most 4096 bytes, and it ends after a complete row, never in the middle of one

#### Scenario: Container line shown
- **WHEN** method `start` of `impl Server` in `src/server/mod.rs` is selected but the `impl Server` row is not
- **THEN** the output shows the `impl Server {` row and then `start` indented 4 spaces under it

### Requirement: repo_map tool
(P1) The `repo_map` tool SHALL accept these arguments:
- `path` (a Location-relative directory or file that restricts the map);
- `focus_paths` (up to 20 paths);
- `focus_symbols` (up to 20 names);
- `tokens` (an integer clamped to 256–8192; default: the model profile's `repo_map.tool_tokens`).

**Permission and visibility:**
- It SHALL be read-only, check the `repo_map` permission with `path` (or `*`) as resource, and use the same default rule as `grep`.
- It SHALL be offered in plan mode.
- Files whose `read` permission for the Session's agent ends in `deny` SHALL be left out of the graph and the output.

**Behaviour:**
- It SHALL refresh the index before ranking.
- It SHALL wait at most 10 s for a running build, then answer from committed state.

**Output:**
- The first line SHALL be `Repository map: <shown> of <indexed> files, <symbols> symbols, ~<tokens> tokens (index <complete|partial|truncated|stale|building>)`.
- A line `not indexed: <path>` SHALL follow for each focus path that is missing or excluded, and then the rendered map.
- An index with no files SHALL return `No indexed files`.
- A build in progress SHALL NOT fail the call.

#### Scenario: Focused map for a weak model
- **WHEN** a local model calls `repo_map` with `focus_paths: ["src/auth/session.rs"]` and no `tokens`, and its profile has `tool_tokens: 2048`
- **THEN** the result starts with the `Repository map:` header and lists `src/auth/session.rs` first, followed by the files it references and the files that reference it, within about 2048 tokens

#### Scenario: Denied file hidden
- **WHEN** the agent's rules end with `{ action: "read", resource: "secrets/**", effect: "deny" }`
- **THEN** no path under `secrets/` appears in any `repo_map` result for that agent

#### Scenario: First call during the initial build
- **WHEN** the model calls `repo_map` 1 s after the server started in a 40 000-file repository
- **THEN** the call returns within about 10 s with the files committed so far and the header says `index building`

### Requirement: Repository map inspection surfaces
(P1) The system SHALL expose the repository map through the CLI and the server API.

**CLI:**
- The CLI SHALL provide `cyber lsp map [<path>] [--focus <path>]... [--symbol <name>]... [--tokens <n>] [--format text|json]`, which prints the map as the tool would render it.
- `--status` SHALL print index status:
  - indexed files by language and excluded files by reason;
  - symbols, last refresh time and build duration;
  - whether the index is complete, partial, truncated or stale;
  - the cache path and size.
- `--rebuild` SHALL re-extract every file of the checkout, and `--clear` SHALL delete the checkout's rows.
- The command SHALL use the running server registered for the Location when there is one. Otherwise it SHALL work in-process on the same cache, without starting a server or creating the session database.

**API:** the server SHALL expose two routes in the `lsp` route group.
- `GET /api/v1/lsp/map` (`v1.lsp.map`) takes the query parameters `path`, `focus` (repeated), `symbol` (repeated) and `tokens`, and returns:
  - `text`;
  - `files: [{ path, rank, symbols: [{ name, kind, line, signature }] }]`;
  - `index: { status, files, symbols, excluded, languages, refreshed_at, build_ms }`.
- `POST /api/v1/lsp/map/rebuild` (`v1.lsp.mapRebuild`) starts a rebuild and returns `202` with the index status.

Both SHALL use the authenticated Location envelope.

#### Scenario: Inspect without a server
- **WHEN** no server is running and the user runs `cyber lsp map --status` in a repository
- **THEN** the status is printed from the cache, and no server process or session database is created

#### Scenario: JSON for scripts
- **WHEN** the user runs `cyber lsp map --focus src/main.rs --format json`
- **THEN** stdout is one JSON object with `text`, `files` and `index`, matching `GET /api/v1/lsp/map`

### Requirement: Repository map data stays local
(P1) The repository index SHALL stay local, as set out below.

**What stays local:** the repository index SHALL stay on the machine that built it. It SHALL NOT be:
- uploaded, shared or synchronized to runners or peers;
- included in Session exports, shares or backups.

A remote runner SHALL build its own index.

**Telemetry and logs:** these SHALL carry only counts, durations and index status. They SHALL NOT carry paths, names or signatures.

**Transcripts:** map text that entered a transcript, as a tool result or in a baseline, SHALL follow that transcript's existing export and redaction rules.

**Untrusted checkouts:** indexing SHALL be permitted, because it parses files and never runs repository code.

#### Scenario: Telemetry without code
- **WHEN** OpenTelemetry export is enabled and an index build finishes
- **THEN** the exported span has file, symbol and duration counts and no path, name or signature attribute

#### Scenario: Untrusted checkout still mapped by the tool
- **WHEN** a checkout is not trusted and the model calls `repo_map`
- **THEN** the map is returned, and no hook, formatter, language server or repository script runs to produce it
