Specified 2026-10-15. Not started. Each numbered section is one increment that ships a working TUI with its own tests. Every increment needs at least one test through a production path: the real `cyber` binary, the App HTTP router, or `TestBackend` driving `crate::view::draw` with responses from the production server. The P2 task (9) is out of scope for the P1 increments.

## 1. Theme tokens, color capability and snapshot harness

- [ ] 1.1 Add `insta` as a dev dependency and a `render(app, width, height, opts)` helper. Opts: theme, `NO_COLOR`, reduced motion, fixed clock. Record baseline snapshots of today's screens at 60×20, 80×24, 120×40 and 160×48 before changing visuals.
- [ ] 1.2 Replace `Theme`'s 10 fields with resolved `Tokens` (the full list from "Theme tokens"). Express the built-ins as embedded theme JSON. Implement `defs`, `{dark, light}` and the derivation rules. Load custom `themes/*.json` (project overrides global).
- [ ] 1.3 Color capability: detect truecolor, 256 or 16 colors; `tui.color`; `NO_COLOR` precedence; a `monochrome` flag with reverse-video selection. Background detection: OSC 11 with a 100 ms timeout, then `COLORFGBG`, re-resolving when a late answer arrives; `tui.theme_mode`.
- [ ] 1.4 Move every existing view (header, messages, tool cards, diffs, overlays, pickers, permission and question prompts) to tokens. Use the `diff_*` backgrounds in diffs.
- [ ] 1.5 Tests: a contrast test for every built-in theme and variant (4.5:1 text, 3:1 muted and focus); a `NO_COLOR` snapshot asserting no cell has `fg`/`bg`; the 256-color mapping; partial-theme derivation; the light variant chosen on a white OSC 11 reply.

## 2. Markdown and syntax highlighting

- [ ] 2.1 Add `pulldown-cmark` (`default-features = false`) and `syntect` (`default-features = false`, features `default-syntaxes`, `parsing`, `regex-fancy`). Measure release binary growth on macOS, Linux musl and Windows MSVC, and record it here. Add `two-face` extras only if total growth stays ≤ 3 MiB; otherwise vendor TypeScript, TOML and Dockerfile syntaxes into a build-time dump. Confirm the Windows CI job builds without a C toolchain change, and that `scripts/measure_startup.py` shows no startup regression.
- [ ] 2.2 New module `markdown.rs`: headings, emphasis, strikethrough, ordered, nested and task lists, quotes, tables with the narrow fallback, OSC 8 links, rules, hanging-indent wrapping. Re-parse only the streaming tail. Block cache.
- [ ] 2.3 New module `highlight.rs`: lazy `SyntaxSet` on a blocking thread, language aliases and extensions, the scope→token table, limits (2,000 lines, 1,000 characters per line), a per-block cache keyed by theme, plain rendering while a block streams, the language label and `surface` background. Apply it to `read` and `write` tool cards.
- [ ] 2.4 Copy code: `y` on a selected block and the palette action (wired in 3.4). Use OSC 52 when no clipboard is available.
- [ ] 2.5 Tests: unit tests for each Markdown construct; the reference answer (numbered bold list and bullets) snapshot at 80 and 120 columns; a Python block colored by scope; a streaming block that stays plain until its fence closes with an unchanged row count; a wide-table fallback; an unknown language that does not fail.

## 3. Composer, status footer, Turn footer marker and palette

- [ ] 3.1 Server: "Assistant message timing". Add `agent`, `started_ms` and `ended_ms` to assistant entries and `promoted_ms` to user entries, from event times. Update the OpenAPI document and SDK types (`scripts/generate_sdk.py`). Test across a server restart.
- [ ] 3.2 Server: implement the specified context meter (`observability-costs` "Context window meter") on `GET /api/v1/sessions/:id` and the `session.usage` live event (`session-runtime` "Usage and cost"), giving last-request prompt tokens, window and `compact_at`. Test through the App HTTP router.
- [ ] 3.3 TUI: composer with the accent bar, mode colors, info line (agent, model, provider, Mode, variant, spinner and `esc interrupt`), placeholder, and 1–8 rows. Status footer with `cwd`, `context` and `cost` (unpriced and lower-bound forms), the key hint from the binding table, toasts on the left, and `tui.statusline.command` unchanged. Remove the old header packing on wide layouts.
- [ ] 3.4 TUI: command palette (Ctrl+P, `palette` context) over built-ins, custom commands, skills, MCP prompts and UI actions, with key hints taken from the binding table.
- [ ] 3.5 TUI: the Turn footer marker (run grouping, duration format, interrupted and failed states, `/timestamps` end time, agent `color`).
- [ ] 3.6 Keybindings: add the `palette`, `sidebar` and `settings` contexts and the defaults `command_palette`, `sidebar_toggle`, `sidebar_focus` and `settings`; hide disabled bindings from hints.
- [ ] 3.7 Tests: marker snapshots for completed, interrupted and failed runs, reloaded from the production server's messages; footer at 60 and 120 columns, including the >85% warning; a rebound palette updating the hint; composer modes.

## 4. Sidebar and responsive layout

- [ ] 4.1 Implement `GET /api/v1/vcs` with branch, upstream, ahead/behind, worktree and dirty flag (the existing P0 "Git awareness" requirement), and `{ "vcs": null }` outside git. Test outside and inside a repository.
- [ ] 4.2 `view/layout.rs` `LayoutPlan` with the 120, 60 and 40 column breakpoints, the drawer mode and the too-small screen. Middle truncation of paths.
- [ ] 4.3 Sidebar sections: title, Context, MCP (collapsible, dots and words, errors, `r` reconnect), LSP (rows or the empty-state sentence), Modified files (from `GET .../diff`, `+N more`), `cwd:branch`, and the connection dot with the version. Event-driven refresh with a re-read on reconnect. Omit a section when its route answers 404 or 503. `tui.sidebar`, `<leader>b`, `/sidebar`, and `<leader>e` focus navigation.
- [ ] 4.4 Tests: snapshots at 160×48 (docked), 100×30 (hidden, then the drawer), 50×20 (compact footer) and 30×8 (too small); a failed MCP server; no LSP; `vcs: null`; a property test over random sizes from 40×10 to 300×100 checking no panic and no overlap; the end-to-end sidebar through the production server with a real MCP stub.

## 5. Settings panel: Status and Config tabs

- [ ] 5.1 `Overlay::Settings` with the tab bar, `Loadable<T>` per tab, the key row, `1`–`4`, Tab and arrow keys, scrolling, `r`, Esc, close on a permission or question request, and the recap kept on top. Commands `/settings`, `/status` and `/config`.
- [ ] 5.2 Status tab from `/health`, the Session, `/models`, the context meter, `/mcp`, `/lsp` and `/vcs`, with the version mismatch warning. Sandbox and account rows appear only when reported.
- [ ] 5.3 Config tab: editable TUI preferences with source labels, saved to `kv.json` with `tui.jsonc` precedence; read-only core settings when the `config` route exists, otherwise the pointer line.
- [ ] 5.4 Tests: snapshots of both tabs at 80 and 120 columns and in `NO_COLOR`; a toggled `sidebar` that persists across an `App` restart; the approval-closes-panel behavior.

## 6. Usage tab and session breakdown

- [ ] 6.1 Store: the `usage_step` and `usage_daily` migration; the projector writes rows in the event transaction (steps, failures, auxiliary usage); a resumable batched backfill (5,000 events per batch) with `backfilling` status; retention folding. Pick `chrono-tz` or `jiff` for local days by measured size. Test that the ledger sums equal the session totals, including descendants and unpriced steps.
- [ ] 6.2 Server: "Session usage breakdown" (`by_model`, `api_duration_ms`, `wall_duration_ms`, `lines_added` and `lines_removed`, `budgets`) on `GET /api/v1/usage?scope=session`. Test through the App HTTP router with two models and a subagent.
- [ ] 6.3 TUI Usage tab: the Session section, compact number and duration formats, the token table and cache hit rate moved from `cost.rs` with its validation intact, and Budget meters (80% and 100% colors, daily reset time and zone). `/usage`, and `/cost` opening this tab; `R` kept as an alias for one release. Remove `Overlay::Cost`.
- [ ] 6.4 Tests: two-model snapshot; an all-unpriced Session showing `—`; partial pricing showing `≥ $x`; no Budget means no limits section; a daily budget at 85%.

## 7. Stats tab and `cyber stats`

- [ ] 7.1 Server: `GET /api/v1/usage/stats` (`period=all|Nd`, `directory`, `tz`): days, totals, favorite model, longest session, active days, streaks, most active day, models, `approximate_days`. Unit-test streak and day-boundary edge cases (DST change, an empty period, today without usage). Add OpenAPI and SDK types.
- [ ] 7.2 CLI: implement `cyber stats` on that route (`--days`, `--project`, `--models`, `--format json`; `--tools` from the existing tool-call projection), replacing the "unavailable" error. Test with the real binary.
- [ ] 7.3 TUI Stats tab: Overview (heatmap fitted to the width with month and weekday labels, quartile levels, legend, metrics in two columns at ≥ 80 columns, token line) and Models (bars); `p` period, `v` subtab, `s` scope; the screen-reader sentence instead of the grid.
- [ ] 7.4 Tests: heatmap snapshots at 70, 120 and 160 columns and in `NO_COLOR` (glyph levels); the period switch; CLI and TUI showing the same numbers for one fixture database.

## 8. Insights and Session recap

- [ ] 8.1 Server: `GET /api/v1/usage/insights` (`window=24h|7d`, `directory`), with the factors `subagent_heavy`, `long_running` and `large_context`, the `basis` rule, subagent groups (agent name, `workflow`, `fork`) and the scope note. The thresholds are constants with tests. Reserve the `workflow` factor for P2.
- [ ] 8.2 TUI: contributing factors in the Usage tab (5% display threshold, advice lines, the Subagents table capped at 8, `d` and `w`, the empty window state).
- [ ] 8.3 Server: "Session recap". Routes `GET|POST .../recap`, the `small` role call with its limits, the `session.recap.generated.1` event, the `through_seq` cache, `usage.recorded.1` purpose `recap`, `409 nothing_to_recap`. Test that a cached recap makes no model call (scripted model).
- [ ] 8.4 TUI: the recap block, `tui.recap` `auto`/`off`, the 30-minute return and focus triggers, `/recap`, hidden on the next prompt, Esc to dismiss. Test that `off` makes no request.
- [ ] 8.5 Accessibility pass over all new surfaces: screen-reader output for the marker, the sidebar omitted, the heatmap sentence; keyboard-only walkthrough tests for the palette, sidebar focus and every tab.

## 9. Provider-reported limits (P2)

- [ ] 9.1 In the provider adapters that receive plan or rate-limit usage windows, record the latest reading per provider and window (label, percent used, reset time, observation time). Never derive one from local usage.
- [ ] 9.2 `GET /api/v1/usage/limits`; the Usage tab meters with reading age, and `d` and `w` between windows. Test that a provider without reports yields an empty list and no meters.

## 10. Documentation and archive

- [ ] 10.1 Update `docs/features.md` and the TUI guide in `docs/local-core-guide.md`: theme JSON format and tokens, `NO_COLOR` and `tui.color`, the sidebar, the palette, Settings tabs and keys, `tui.recap`, and `cyber stats`.
- [ ] 10.2 Run `openspec validate improve-tui-experience --strict` and `python3 scripts/spec_lint.py`. Archive the change, then refresh the ROADMAP inventory table with `python3 scripts/spec_inventory.py`.
