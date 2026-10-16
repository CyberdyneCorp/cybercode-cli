## Why

The TUI works, but it reads like a debug console next to OpenCode and Claude Code. Measured against the reference screens the user supplied (OpenCode's session view, and Claude Code's `/stats` and Usage tabs):

- **Rendering.** `view/messages.rs` is a line-based "Markdown-lite": `#` headings, `-`/`*` bullets, inline `code` and `**bold**`. It has no ordered or nested lists, no tables, quotes, links or italics, and it ignores the P0 requirement for syntax-highlighted code blocks: code is one flat `code` color.
- **Theme.** `theme.rs` has 10 colors per theme, with no surfaces, diff, syntax or data tokens. There is no light/dark variant, no color-depth fallback, and `NO_COLOR` is ignored.
- **Chrome.** The header packs the title, model, Mode, raw token counts and cost into one row. The composer is a bordered box. The status row only shows key hints. There is no turn summary, no sidebar and no command palette, even though the `tui` spec already refers to "the command palette".
- **Usage views.** Only `/cost` exists, as a small overlay. `/status`, `/usage` and `/stats` are P1 requirements in `observability-costs`, and `cyber stats` is a P0 requirement; none is implemented. The usage API answers only `scope=session` totals: no per-model breakdown, durations, lines changed, or any cross-Session history.

## What Changes

The change ships in eight increments, and each one leaves a working TUI (see tasks.md):

1. **Theme tokens and color capability.** About 40 semantic tokens with derivation rules for partial themes, `{dark, light}` variants with OSC 11/`COLORFGBG` detection, truecolor/256/16/none output, and `NO_COLOR` honored. Every view moves to tokens. A snapshot test harness at 60, 80, 120 and 160 columns, plus a `NO_COLOR` run.
2. **Markdown and code.** `pulldown-cmark` for CommonMark+GFM, and `syntect` (pure-Rust `fancy-regex` backend) for highlighting finished code blocks, mapped onto `syntax_*` tokens.
3. **Composer, status footer, Turn footer marker and command palette.** An accent-bar composer with an agent·model·Mode info line. A footer showing `cwd`, `29.5K (22%) · $0.00  ctrl+p commands`. A `▣ Build · amini-coder · 11.3s` marker after each finished run. A Ctrl+P palette. Server side, assistant entries gain `agent`, `started_ms` and `ended_ms`, and the specified context meter is implemented.
4. **Sidebar and responsive layout.** A right sidebar (Context, MCP, LSP, modified files, `cwd:branch`, version) that docks at 120 columns or more and opens as a drawer below that. Breakpoints down to 40×10.
5. **Settings panel: Status and Config tabs.** `/settings`, `/status` and `/config`.
6. **Usage tab.** A ledger of provider steps, a session usage breakdown (by model, API and wall durations, lines added and removed, Budgets), and limit meters only for configured Budgets. `/usage`, and `/cost` moves into it.
7. **Stats tab and `cyber stats`.** `GET /api/v1/usage/stats`: heatmap, streaks, favorite model, token breakdown, period and scope switching. The CLI renders the same response.
8. **Insights and recap.** `GET /api/v1/usage/insights` (subagent-heavy, long-running, >150k context, and P2 workflow shares, plus a Subagents table), and the Session recap (`POST /api/v1/sessions/:id/recap`, `※ recap:` line, `tui.recap`).

P2 adds provider-reported limits (`GET /api/v1/usage/limits`). They are shown only when a provider reports them, never estimated.

## Capabilities

### Modified Capabilities
- `tui`: 17 ADDED requirements (16 P1, and "Usage limit meters" with a P2 clause). 5 MODIFIED: "Message and part rendering", "Status line" (new default fields `cwd, context, cost`, placed below the composer, palette hint), "Themes and appearance", "Keybindings" (new contexts and defaults), "Accessibility" (3:1 muted and focus contrast, no color-only meaning, keyboard reachability, screen-reader forms of the new views).
- `observability-costs`: ADDED "Usage step ledger", "Session usage breakdown", "Usage statistics API", "Usage insights" (P1) and "Provider-reported limits" (P2). MODIFIED "Usage commands" (`/stats` added; the slash commands open Settings tabs).
- `session-runtime`: ADDED "Assistant message timing" and "Session recap" (P1).
- `server-api`: MODIFIED "Session and prompt routes" (P1 `GET|POST .../recap`).
- `cli-commands`: MODIFIED "Stats command" (renders `GET /api/v1/usage/stats`).

`configuration` needs no change: the new `tui.*` keys belong to `tui.jsonc`, which "TUI config document" already covers. The new routes stay inside the existing `session` and `usage` route groups.

## Impact

- **Code.** `crates/cyber-tui` (theme, view, new `markdown`, `highlight`, `sidebar`, `settings` and `palette` modules), `crates/cyber-server` (usage ledger projection, usage routes, recap, message timing, context meter), `crates/cyber-store` (ledger migration and backfill), `crates/cyber-cli` (`cyber stats`), `sdk/openapi.json` and the generated TypeScript types.
- **Dependencies.** `pulldown-cmark` (pure Rust) and `syntect` with `default-features = false` and `default-syntaxes`, `parsing` and `regex-fancy` (pure Rust, no Oniguruma). Optionally `two-face` for extra syntaxes, if it stays within the binary budget. Dev only: `insta` for snapshots. Release binary growth budget: ≤ 3 MiB, measured in task 2.1.
- **Data model.** A new `usage_step` table and a `usage_daily` aggregate, with a backfill migration. Existing usage totals keep their meaning. A new durable event type, `session.recap.generated.1`.
- **Data gaps closed by this change.** No per-model or per-day usage query exists. Assistant messages carry no agent or timing. The context meter ("Context window meter", P1) and the `session.usage` live event (`session-runtime` "Usage and cost", P0) are specified but not implemented. `GET /api/v1/vcs` (`vcs-integration` "Git awareness", P0) is not implemented, so there is no branch for the sidebar.
- **Remaining gaps, not filled with fake data.** Provider quota windows (P2, only when reported). Sandbox and account state in the Status tab (shown only when the server reports them). Effective core config in the Config tab (needs the unimplemented `config` route group). Usage on other machines or provider dashboards is never included, and the insights say so.
- **Canonical inventory after archive.** +24 requirements: `tui` +17, `observability-costs` +5 (one P2), `session-runtime` +2.
