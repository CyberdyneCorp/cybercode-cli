## Why

On a large repository the agent spends many Turns and tokens just finding where things are: `glob` for names, `grep` for a symbol, `read` of three wrong files, then the right one. Weaker and local models suffer most. They call search tools less well, give up early, or edit the first file that looks plausible. What exists today does not cover this:

- `glob`, `grep` and `list` (`builtin-tools`) find text, not structure. They know nothing about which files matter.
- The `lsp` tool (`code-intelligence`) has `workspace_symbols`, but only when a language server is installed and started for each language. Its default permission is `ask`, and it returns at most 50 rows for one query. It gives no overview.
- Nothing in the system prompt tells the model what the repository contains.

Aider showed a cheap fix that works across models: a **repository map**. It is a compact, ranked list of the repository's files and their key definitions (signature lines), fitted to a token budget. Ranking runs PageRank over the graph of "file A references a name defined in file B", personalized towards the files and names the agent is working on. We take that behaviour (not its code) and fit it to our runtime. The hard constraint is prompt caching: `system-context` keeps the baseline byte-stable within a Context Epoch, so a map that changes on every edit must not sit in the cached prefix.

## What Changes

- **Repository index** (`code-intelligence`, added "Repository map configuration", "Repository map file selection", "Repository map symbol extraction", "Repository map index storage" and "Repository map build limits and freshness").
  - Definitions and references are extracted with tree-sitter tag queries for a fixed, bundled language set. `tree-sitter` 0.27 is already linked into `cyber-tools` for shell analysis.
  - Files come from the same ignore-aware walk as `grep`/`glob`. Generated and vendored files, files over 1 MiB and binaries are skipped.
  - The index is a local, rebuildable cache in `<cache>/repo-map/index.db`. Extracted tags are stored by content hash, so worktrees of one repository share them. Changed files are found by size and mtime, then hashed.
  - Builds are bounded: 50 000 files, 250 ms per file and 120 s per build. When a bound is hit, the index is usable and marked `partial` or `truncated`.
- **Ranking and rendering** (`code-intelligence`, added "Repository map ranking" and "Repository map rendering and token budget").
  - Personalized PageRank runs over the file reference graph, with fixed weights and fixed iteration limits. The output is deterministic and byte-identical for the same index and inputs.
  - Output is path lines with `line: signature` rows. It is fitted to a token budget at 4 characters per token.
- **`repo_map` tool** (`code-intelligence`, added "repo_map tool"; `builtin-tools`, modified "Capability-owned tool catalog"). It is read-only and takes `path`, `focus_paths`, `focus_symbols` and `tokens`. It refreshes the index first. It is allowed in plan mode, and files the agent may not `read` are excluded.
- **Map in the initial context, without breaking caching** (`system-context`, added "Epoch-pinned Context Sources" and "Repository map Context Source").
  - A new Context Source property, *epoch-pinned*: the source is observed only when a baseline is rendered and is skipped by Safe Boundary reconciliation. It never produces mid-conversation update messages.
  - `core/repo-map` is the first such source. It is a small unpersonalized map (default 1 024 tokens), present only in trusted checkouts. A fresh one arrives with the next Epoch, for example after compaction.
- **Per-model budgets** (`provider-catalog`, added "Repository map budgets in model profiles"). The model behavior profile from `improve-model-protocol-fidelity` gains `repo_map: { context_tokens, tool_tokens }`. Both are capped by a share of the model's context window, so small local models get a smaller map.
- **Inspection** (`code-intelligence`, added "Repository map inspection surfaces"; `tui`, added "Repository map view").
  - `cyber lsp map` shows the map or the index status, and `--rebuild` and `--clear` manage the index.
  - The server adds `GET /api/v1/lsp/map` and `POST /api/v1/lsp/map/rebuild`.
  - The TUI adds `/map`.
- **Privacy** (`code-intelligence`, added "Repository map data stays local"). The index never leaves the machine. Telemetry carries counts and durations only.
- **Config** (`configuration`, modified "Top-level keys"). Adds the top-level key `repo_map`.

## Non-goals

- **Semantic (embedding) search over code.** It is deferred to a later change, `add-semantic-code-search`, proposed for P4. The reasons:
  - It needs an embedding provider. A remote one sends code off the machine. A local one means a model download of hundreds of MB, outside the single static binary.
  - The index is large, roughly 100–200 MB for 100 000 chunks, and edits must be re-embedded.
  - We should first measure whether map + `grep` + `lsp` leaves a real gap (task 9.2).
- **Using LSP or ctags to build the map.** See design.md D1. The `lsp` tool stays the precise navigation tool. The map never starts a language server.
- **A file watcher daemon.** Freshness comes from refresh on use and from Cyber's own edit tools (D5).
- **Bundling every grammar.** SystemVerilog, Ruby, PHP, C#, Kotlin, Swift and others appear as paths without symbols until a later change adds them within the binary-size budget (D1).
- **Maps across several repositories**, and maps of prose (Markdown headings, docs).
- **Call graphs or impact analysis for refactoring.** The reference graph is used only for ranking.

## Capabilities

### New Capabilities

None. The repository map is part of `code-intelligence`, which already owns the `lsp` command group and the `lsp` route group.

### Modified Capabilities

- `code-intelligence`, all ADDED:
  - "Repository map configuration";
  - "Repository map file selection";
  - "Repository map symbol extraction";
  - "Repository map index storage";
  - "Repository map build limits and freshness";
  - "Repository map ranking";
  - "Repository map rendering and token budget";
  - "repo_map tool";
  - "Repository map inspection surfaces";
  - "Repository map data stays local".
- `system-context`, ADDED: "Epoch-pinned Context Sources" and "Repository map Context Source".
- `provider-catalog`, ADDED: "Repository map budgets in model profiles". It extends "Model behavior profiles", which `improve-model-protocol-fidelity` adds, so that change must be archived first.
- `tui`, ADDED: "Repository map view".
- `builtin-tools`, MODIFIED: "Capability-owned tool catalog" gains `repo_map` (code-intelligence).
- `configuration`, MODIFIED: "Top-level keys" gains `repo_map`.

The two MODIFIED requirements are shared registries. Other open changes (for example `add-binary-hex-editor`) modify the same catalog requirement. Whichever change is archived later re-applies its own line on top of the current text (design.md → Migration).

## Impact

- **Code:**
  - `crates/cyber-tools/src/repo_map/` (new): `walk.rs`, `extract.rs`, `queries/*.scm`, `store.rs`, `graph.rs`, `rank.rs`, `render.rs`, plus the `repo_map` tool in `tools/repo_map.rs`.
  - `crates/cyber-tools/Cargo.toml`: grammar crates for Rust, TypeScript/TSX, Go, C, C++, Java and Solidity. Bash, Python and JavaScript are already there.
  - `crates/cyber-core`: the `repo_map` config schema, the `pinned` flag on Context Sources, and the `core/repo-map` source.
  - `crates/cyber-llm/src/catalog/profile.rs`: the `repo_map` profile field.
  - `crates/cyber-server`: index build on Location services start, the two routes, and dirty marks from the edit tools.
  - `crates/cyber-cli`: `cyber lsp map`. `crates/cyber-tui`: `/map`.
- **Data:** a new cache database `<cache>/repo-map/index.db`, which can be deleted at any time. There are no new durable events and no change to the session store.
- **API, SDK and CLI:**
  - two routes in the existing `lsp` group, so OpenAPI and the TS SDK are regenerated;
  - a new `lsp map` subcommand;
  - a new built-in tool, so the tool schema bundle at `GET /api/v1/tools/schema` changes.
- **Binary size:** about +6–10 MiB for the grammars. A CI gate fails above +12 MiB (D1).
- **Licensing:** the grammar crates and their `tags.scm` queries are MIT and listed in third-party notices. No code is taken from Aider (Apache-2.0); we reimplement only its observable behaviour.
- **Docs:** `docs/local-core-guide.md` (repository map, `core/repo-map`, epoch-pinned sources), `docs/features.md`, `docs/cli.md` and a ROADMAP entry. These are written when the change is implemented, not in this spec change.
