## Context

The TUI (`crates/cyber-tui`, ratatui 0.30, crossterm 0.29) talks to the server only through `/api/v1` routes and the event stream, so anything it shows must have a public source. A survey on 2026-10-15 found:

| Need | Source today | Status |
|---|---|---|
| Markdown, code highlight | `view/messages.rs` `markdown()` / `prose()` / `inline()` | Line-based subset, no highlighter |
| Theme | `theme.rs`: 10 colors × 12 themes, `kv.json` `theme` | No surfaces, diff, syntax or data tokens; no light/dark; no `NO_COLOR` |
| Session cost and tokens | `GET /api/v1/sessions/:id` `totals`, `children_*`; `GET /api/v1/usage?scope=session` | Totals only; the other scopes answer `503` |
| Per-model usage, step timing | `session.step.started.1` (agent, provider, model) and `session.step.ended.1` (usage, cost); event `time` column | Stored, not queryable; no per-model or per-day query |
| Context % | `GET /api/v1/models` `context_limit`; last step usage | Meter specified (P1) but not exposed; `session.usage` live event not emitted |
| MCP status | `GET /api/v1/mcp` (`connecting`/`connected`/`disabled`/`failed`/`needs_auth`/…), `McpStatusUpdate` events | Available |
| LSP status | `GET /api/v1/lsp` (`starting`/`connected`/`broken`), `LspTransition` | Available |
| Modified files, lines changed | `GET /api/v1/sessions/:id/diff` (`FileDiff` additions/deletions); `session.diff.1` per message | Available per Session |
| Branch | `GET /api/v1/vcs` (`vcs-integration`, P0) | Not implemented |
| Workflow/subagent attribution | Session `workflow_run_id`, `subagent_name`, `parent_id`, `agent` | Available |
| Budgets | `session_budget` table, `budgets.*` config | Available |
| Provider quota windows | Only `retry-after` parsing in `cyber-llm/src/error.rs` | Not captured |
| Server version | `GET /api/v1/health` | Available |

## Goals / Non-goals

Goals: the visual quality of the reference screens; one data contract shared by the TUI, `cyber stats` and the SDK; honest numbers (unknown ≠ zero, no invented limits); every increment shippable on its own.

Non-goals: mouse-driven layout editing; a GUI-style settings editor for core config (needs the `config` route group); per-agent cost forecasting; plugin-provided panes (P4 "Mod panes").

## Decisions

### Theme tokens over palette slots
Views take `&Tokens` (a resolved struct of `ratatui::style::Color`) instead of `Theme`'s 10 fields. Resolution order: theme JSON value → `defs` reference → `{dark, light}` pick → derivation rule from `text`/`background`/`accent`. Built-in themes are compiled into the binary as the same JSON documents, so custom and built-in themes take one path. Color depth is applied once at resolution time (`Rgb` → nearest `Indexed` or ANSI 16), not in every view. `NO_COLOR` turns every token into `Color::Reset` and sets a `monochrome` flag. The few views that need a non-color fallback (selection, heatmap, status dots, meters) check this flag. A unit test computes the WCAG contrast for every built-in theme and variant: `text` ≥ 4.5:1 and `text_muted` and `border_active` ≥ 3:1 against `background`.

Background detection sends OSC 11 at startup with a 100 ms timeout, and only when stdout is a TTY. It never blocks the first frame: the first frame uses `dark` (or `COLORFGBG`), and a late answer re-resolves the tokens.

### Markdown: pulldown-cmark
`pulldown-cmark` is pure Rust, small, and has GFM tables, task lists and strikethrough behind options. We disable `default-features` (no `getopts` binary or SIMD feature negotiation). Rendering is a pure function `render(&str, width, &Tokens) -> Vec<Line>` with a block cache keyed by `(block_hash, width, theme_id)`. Streaming: the text is split at the start of the last top-level block that is not yet closed, so only the tail is re-parsed per delta. Wrapping keeps a per-line hanging indent; `view/wrap.rs` gains a `continuation_prefix`.

### Highlighter: syntect with fancy-regex, not tree-sitter-highlight
| | `syntect` (`regex-fancy`) | `tree-sitter-highlight` |
|---|---|---|
| Build | Pure Rust; no C toolchain | One C parser per language, built by `cc`, plus `highlights.scm` queries we must vendor |
| Windows | Builds with plain `cargo` on MSVC and GNU targets; no Oniguruma | Works (we already build `tree-sitter-bash`/`-powershell` for `cyber-tools`), but every grammar adds MSVC compile time and C warnings to triage |
| Size | One compressed syntax dump (~0.5–1 MiB for the default ~75 languages, ~2–3 MiB with `two-face` extras) | Grows per language: large grammars (TypeScript, C++) are each several MiB of object code; 20 languages → over 10 MiB |
| Coverage | Sublime syntaxes: broad, adding one is a data file | Per-grammar and query availability; uneven |
| Accuracy | Regex-based; good for display | Parse-tree based; better, incremental |
| Speed | `fancy-regex` is slower than Oniguruma; fine for finished blocks with caching | Fast |

The TUI only needs display-quality coloring of finished blocks, so binary size, a pure-Rust Windows build and language breadth matter more than parse accuracy. Configuration: `syntect = { default-features = false, features = ["default-syntaxes", "parsing", "regex-fancy"] }`. There are no bundled themes: syntect scopes map onto our `syntax_*` tokens through a ~30-entry scope-prefix table (`comment` → `syntax_comment`, `keyword`/`storage` → `syntax_keyword`, `entity.name.function` → `syntax_function`, `string` → `syntax_string`, `constant.numeric` → `syntax_number`, `entity.name.type`/`support.type` → `syntax_type`, …). So highlighting follows every theme for free. Sublime's default set lacks TypeScript, TOML and Dockerfile. We add `two-face`'s fancy-regex-compatible extra set only if the measured release growth stays ≤ 3 MiB. Otherwise we vendor just those three `.sublime-syntax` files into a build-time dump. The `SyntaxSet` loads lazily on a blocking thread the first time a code block finishes, so startup time is unchanged (checked with `scripts/measure_startup.py`). The existing `tree-sitter` dependency stays in `cyber-tools` for shell parsing and is not reused for display.

### Turn footer marker from durable data
A run is the assistant entries between one user entry and the next. The marker needs the agent and the run's first start and last end, so `session-runtime` gains "Assistant message timing": `agent`, `started_ms`, `ended_ms` and the user entry's `promoted_ms`, taken from the event `time` column the projector already reads. The TUI never times runs with its own clock, so the marker is the same after reload, on another client, or after a restart. The model display name comes from the cached `GET /api/v1/models`.

### Footer, composer and palette
Layout rows, top to bottom: optional header (narrow layouts only), optional recap block, transcript, queued items, composer (accent bar, 1–8 text rows and an info line), status footer (1 row). The run-state hints that used to sit in the status row move to the composer info line, so the footer stays stable. The footer hint is rendered from the active keybinding table, so it can never advertise a dead key. Ctrl+P is unused today, so the palette takes it. Reusing `fuzzy.rs` and the picker overlay keeps the palette at about 150 lines.

### Sidebar and responsive layout
`view/layout.rs` computes a `LayoutPlan { header, recap, transcript, sidebar: Docked|Drawer|Hidden, composer, footer }` from the size and preferences, and every draw function takes its `Rect` from the plan. Breakpoints are 120, 60 and 40 columns. Sidebar data lives in `app.sidebar: SidebarState`, filled by `LoadSidebar(Part)` actions (MCP, LSP, the Session diff, VCS, health, parent) and updated from the `mcp.status.changed.1`, `mcp.tools.changed.1` and `session.diff.1` events; no LSP event reaches the instance stream, so LSP and the branch are re-read when the Session goes idle. All of them are re-read on SSE reconnect. When a route answers 404 or 503, the sidebar section is omitted (the `vcs` case today) instead of showing "unknown".

### One usage ledger instead of event scans
All-time stats over the raw `event` table would scan every step event on each open. The `usage_step` projection (one row per step or auxiliary call, written in the projector transaction) makes every view one indexed `GROUP BY`: per model, per local day, per root Session, per agent group. The existing session `cost`/`*_tokens` columns stay authoritative for totals, and a test asserts that the ledger sums equal them. Local days are computed in SQL from `started_ms` with a client-supplied IANA zone. Only `chrono` is in `Cargo.lock` today, so zone offsets need either `chrono-tz` (pure Rust, embeds the tz database, about 0.5 MiB more) or `jiff` (which reads the system tz database and bundles it on Windows). Task 6.1 picks one by measured size, and the server falls back to its own zone when the name is unknown. Retention folds old rows into `usage_daily` by UTC day, and responses flag `approximate_days` when the period reaches into folded data.

Streaks, favorite model and most active day are computed on the server, so `cyber stats`, the TUI and the SDK cannot disagree.

### Insights are characteristics, not a breakdown
Each factor is a share of the window total, and factors overlap (a Session can be long-running and subagent-heavy). The UI says so, as the reference does. The basis is cost when everything in the window is priced, else tokens, and the UI labels which. Thresholds (50% descendants, 8 hours, 150,000 prompt tokens) are constants in one module with tests. The workflow factor is P2 and appears only once Workflow Runs exist.

### Limits: configured Budgets now, provider windows only when reported
Budgets are real limits that the user configured and the runtime enforces, so meters for them are honest P1. Provider plan windows (session or weekly percent, reset time) exist only for some providers and only in their response headers or usage endpoints. P2 records the latest reading per provider in the adapter and exposes it. When nothing reports, the section disappears. We never extrapolate from local usage, because a local estimate looks like a quota but would be wrong for any multi-device user.

### Recap on the server
The recap is generated by the server (`small` role, no tools, ≤ 120 output tokens), stored as a durable event, and cached by `through_seq`. Every client (TUI, web, SDK) can show it, re-opening never pays twice, and its usage is attributed like titles. The TUI asks only on return (≥ 30 minutes idle or away) or on `/recap`, and `tui.recap: off` disables it. Privacy is the same as title generation: the Session content goes to the configured small model.

### Settings panel
One `Overlay::Settings { tab, sub, scroll }` replaces the `Overlay::Cost` overlay. `CostView`'s parsing and validation (`cost.rs`) move into the Usage tab model unchanged, so its consistency checks keep running. Each tab owns a `Loadable<T>` (idle, loading, loaded or failed, plus a generation counter, the pattern `CostView` already uses), so stale responses are dropped. Tabs render with a scrollable `Paragraph` built from pure `fn lines(&Model, width, &Tokens) -> Vec<Line>` functions, which are what the snapshot tests target.

### Keyboard and accessibility
Every new surface has a keybinding context (`palette`, `sidebar`, `settings`) in the existing merge model. The focus marker is reverse video, plus `›` in monochrome. Screen-reader mode drops the sidebar, prints the marker as a sentence and replaces the heatmap with a sentence. Nothing relies on color alone: dots have words, meters have percentages, heat levels have glyphs.

### Snapshot tests
`tests.rs` already renders through `TestBackend` at 100×30. We add `insta` (dev only) and a `render(app, w, h, opts) -> String` helper that serializes symbols plus a compact style legend for chosen rows. Fixtures: a fixed clock, fixed versions, reduced motion (no spinner frames), a deterministic theme (`cyber` dark), and canned API JSON. Matrix: 60×20, 80×24, 120×40 and 160×48, with `NO_COLOR` and the `github-light` theme at 120×40. Scenes: transcript with Markdown and code, running state, sidebar (connected, failed MCP, no LSP), each Settings tab, and the too-small screen. A property test renders random sizes from 40×10 to 300×100 and asserts that nothing panics and no region overlaps another.

## Risks / Trade-offs

- **Binary size.** syntect and its dumps add weight. Mitigation: no bundled themes, a 3 MiB budget measured in CI, and the fallback to the default set plus three vendored syntaxes.
- **`fancy-regex` slowness on pathological input.** Mitigation: highlight finished blocks only, cap at 2,000 lines and 1,000 characters per line, cache results, and run on a blocking thread.
- **Ledger backfill time on large databases.** Mitigation: the backfill runs in batches of 5,000 events as a resumable data migration (`data_migration` table). Until it finishes, stats report `backfilling: true` and the Stats tab shows `Indexing history…`.
- **Recap cost.** One small-model call per return. Mitigation: cache by `through_seq`, the 30-minute threshold, and `tui.recap: off`.
- **Spec drift with `/cost`.** `/cost` moves into the Usage tab. The data and checks are unchanged, and the "Usage commands" requirement is restated to say so.

## Migration

- `kv.json` `theme` values keep working. Theme names are unchanged.
- `tui.statusline.fields` users keep their fields. Only the default changes.
- The `/cost` overlay becomes the Usage tab, and `R` refresh is now `r`. Both keys work for one release.
- The database migration adds the `usage_step` and `usage_daily` tables and runs the backfill. A downgrade ignores the tables.
