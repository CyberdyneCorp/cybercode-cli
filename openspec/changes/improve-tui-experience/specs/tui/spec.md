## ADDED Requirements

### Requirement: Theme tokens
(P1) Every theme SHALL resolve to one set of semantic color tokens, and views SHALL use only these tokens, never literal colors:
- surfaces: `background`, `surface` (composer and panels), `surface_alt` (sidebar), `border`, `border_active`, `selection`
- text: `text`, `text_muted`, `text_subtle`
- roles: `accent`, `secondary`, `user`, `success`, `warning`, `error`, `info`
- diffs: `diff_added`, `diff_removed`, `diff_added_bg`, `diff_removed_bg`, `diff_context`
- Markdown: `md_heading`, `md_link`, `md_code`, `md_quote`, `md_rule`
- syntax: `syntax_comment`, `syntax_keyword`, `syntax_function`, `syntax_variable`, `syntax_string`, `syntax_number`, `syntax_type`, `syntax_operator`, `syntax_punctuation`
- data: `heat_0` to `heat_4`, `meter_fill`, `meter_empty`

A theme JSON file (`themes/*.json`) SHALL have the form `{ "$schema"?, "defs"?: { name: "#rrggbb" }, "theme": { token: "#rrggbb" | defName | { "dark": value, "light": value } } }`. A token missing from a theme SHALL be derived from the theme's `text`, `background` and `accent` (for example `surface` = `background` blended 6% toward `text`, `heat_1..4` = `accent` at 25/50/75/100% over `background`). A theme with `{dark, light}` values SHALL render the variant that matches the terminal background. The built-in `cyber` theme SHALL define both variants. `tui.theme_mode` SHALL accept `auto` (default), `dark` and `light`. In `auto` the system SHALL detect the background with an OSC 11 query (100 ms timeout), then `COLORFGBG`, and SHALL use `dark` when neither answers.

#### Scenario: Light terminal picks the light variant
- **WHEN** the theme is `cyber`, `tui.theme_mode` is `auto`, and the terminal answers OSC 11 with a white background
- **THEN** the TUI renders the `cyber` light variant, with body text meeting 4.5:1 contrast against the background

#### Scenario: Partial custom theme
- **WHEN** `themes/corp.json` defines only `text`, `background` and `accent`
- **THEN** the theme loads, and `surface`, `text_muted` and `heat_1..4` are derived instead of failing

### Requirement: Color capability and NO_COLOR
(P1) The system SHALL render colors at the terminal's capability: 24-bit when `COLORTERM` is `truecolor` or `24bit`, else the nearest xterm-256 color when `TERM` contains `256color`, else the nearest of the 16 ANSI colors. `tui.color` SHALL override detection with `auto` (default), `truecolor`, `256`, `16` or `none`. When the `NO_COLOR` environment variable is set to a non-empty value, or `tui.color` is `none`, the system SHALL emit no foreground or background colors. It SHALL convey state only with text attributes (bold, italic, underline, reverse), glyphs and words: selection and focus in reverse video, status dots paired with a status word, and heatmap levels as glyph density (`·░▒▓█`). `NO_COLOR` SHALL take precedence over `tui.color`.

#### Scenario: NO_COLOR keeps the UI usable
- **WHEN** the TUI starts with `NO_COLOR=1` and a Settings tab is selected
- **THEN** no cell carries a foreground or background color, and the selected tab is drawn in reverse video

#### Scenario: 256-color terminal
- **WHEN** `COLORTERM` is unset and `TERM` is `xterm-256color`
- **THEN** every theme color is mapped to its nearest xterm-256 index

### Requirement: Markdown rendering
(P1) The system SHALL render assistant text as CommonMark with these GitHub extensions: tables, task lists and strikethrough. It SHALL render:
- headings in bold `md_heading`, with H1 and H2 followed by a blank line
- **bold**, *italic*, ~~strikethrough~~ and `inline code` in `md_code`
- ordered lists with their own numbers, bullets as `•`, nested lists indented 2 columns per level, and task items as `☐` or `☑`
- block quotes with a `▎` gutter in `md_quote`
- tables with box-drawing borders; a table wider than the transcript SHALL fall back to one `header: value` line per cell
- links as their text, with the URL as an OSC 8 hyperlink when the terminal supports it and otherwise appended in `text_subtle`
- thematic breaks as a `md_rule` line across the transcript width

Wrapped list items and quotes SHALL keep their hanging indent. While a message streams, the system SHALL re-parse only from the start of the last unfinished block, so earlier blocks never reflow. Raw HTML SHALL be shown as literal text.

#### Scenario: Numbered list with bold terms
- **WHEN** the assistant returns `1. **Memoization:** Results are cached`
- **THEN** the line shows `1. ` followed by `Memoization:` in bold and the rest in `text`, with no asterisks

#### Scenario: Wide table on a narrow terminal
- **WHEN** a 6-column table needs 140 columns and the transcript is 80 columns wide
- **THEN** each row is rendered as `header: value` lines instead of a clipped table

### Requirement: Syntax-highlighted code blocks
(P1) The system SHALL highlight fenced code blocks, and code in tool cards for `read` and `write`, with a bundled pure-Rust highlighter mapped onto the `syntax_*` tokens. The language SHALL come from the fence info string (aliases such as `ts`, `py`, `sh` and `rs` resolved), else from the file extension for tool output. Unknown languages SHALL render in `md_code` without failing. A code block SHALL render with a `surface` background, a 2-column left padding, and a dim language label on its first row. Highlighting SHALL run only for finished blocks; the streaming tail of an open block SHALL render plain until the closing fence arrives. Blocks longer than 2,000 lines or lines longer than 1,000 characters SHALL render plain. Highlight results SHALL be cached per block and theme. Code SHALL never be re-wrapped mid-token for highlighting; long lines wrap at the transcript width with a `↪` continuation marker in `text_subtle`. Pressing `y` on a selected code block, or using the command palette's "Copy last code block", SHALL copy the code without padding or markers, using OSC 52 when no native clipboard is available.

#### Scenario: Python block highlighted
- **WHEN** the assistant returns a fenced `python` block containing `# comment` and `for i in range(10):`
- **THEN** the comment renders in `syntax_comment` italic and `for` in `syntax_keyword`

#### Scenario: Streaming block stays plain until closed
- **WHEN** a fenced block is still streaming
- **THEN** its lines render in `md_code` and are highlighted once the closing fence arrives, with no change to the row count

### Requirement: Turn footer marker
(P1) After the last assistant message of each finished run (the Turns that answer one promoted user input), the system SHALL render one marker line: `▣ <Agent> · <model> · <duration>`. `<Agent>` is the agent's name with its first letter upper-cased, colored with the agent's configured `color` or else `accent`. `<model>` is the display name from `GET /api/v1/models`, falling back to the model ID. `<duration>` is the wall time from the first Turn's `started_ms` to the last Turn's `ended_ms`. Durations SHALL be formatted as `850ms`, `11.3s`, `4m 05s` or `1h 02m`. An interrupted run SHALL end with `· interrupted` in `warning`, and a failed run with `· failed` in `error`. The marker SHALL be built only from durable message data, so it is the same after a reload, and SHALL not be shown while the run is still active. `/timestamps` SHALL add the local end time (`14:02`) to the marker.

#### Scenario: Marker after a finished answer
- **WHEN** the `build` agent on model `amini-coder` finishes a run whose Turns span 11.3 seconds
- **THEN** the transcript shows `▣ Build · amini-coder · 11.3s` below the answer

#### Scenario: Interrupted run
- **WHEN** the user presses Esc 4.2 seconds into a run
- **THEN** the marker reads `▣ Build · amini-coder · 4.2s · interrupted` in the warning color

### Requirement: Composer appearance
(P1) The composer SHALL be drawn as a `surface` block with a 1-column left accent bar `┃` and no other border. The accent bar color SHALL show the input mode: the current agent's color for prompts, `warning` for `!` shell input, `info` for `/` commands and `secondary` for `#` memory input. The last row of the composer SHALL be an info line, `<Agent> · <model> <provider name>`, followed by the Mode label (and the variant when the model has variants), in `text_muted`. While a Drain runs, the info line SHALL end with a spinner (a static `…` under reduced motion) and `esc interrupt`. An empty composer SHALL show the placeholder `Ask anything… "/" commands · "@" files · "!" shell` in `text_subtle`. The text area SHALL grow from 1 to 8 rows and then scroll. Pending paste and image chips SHALL render in `accent` on `selection`.

#### Scenario: Shell mode recolors the bar
- **WHEN** the user types `!` as the first character
- **THEN** the accent bar turns to the warning color and the info line shows `shell`

#### Scenario: Running state in the info line
- **WHEN** a Drain is running
- **THEN** the composer info line ends with a spinner and `esc interrupt`

### Requirement: Command palette
(P1) The system SHALL open a command palette with the `command_palette` keybinding (default Ctrl+P), and with `/` followed by Tab in an empty composer. It SHALL list every built-in TUI command, custom command, skill and MCP prompt, plus UI actions (toggle sidebar, open settings tabs, switch theme, copy last code block, toggle reasoning). Each row SHALL show the name, a one-line description and the current key binding. Typing SHALL filter by fuzzy match, Enter SHALL run the selected entry, and Esc SHALL close the palette without side effects.

#### Scenario: Open settings from the palette
- **WHEN** the user presses Ctrl+P, types `usage` and presses Enter
- **THEN** the Settings panel opens on the Usage tab

### Requirement: Session sidebar
(P1) The system SHALL provide a right sidebar, toggled by `<leader>b` and `/sidebar`, with `tui.sidebar` set to `auto` (default), `show` or `hide`. In `auto` it SHALL be visible when the terminal is at least 120 columns wide. Its width SHALL be 30% of the terminal, clamped to 32..48 columns. From top to bottom it SHALL show:
- the Session title, and the parent title for child Sessions
- **Context**: used tokens, percent of the context window used (`warning` above 85%), and cost spent (`—` when unpriced, `≥ $x` when partly unpriced)
- **MCP**: a collapsible list (`▼`/`▶`), with one row per configured server: a status dot and the status word (`Connected`, `Connecting`, `Failed`, `Needs auth`, `Disabled`); a failed server shows its error, truncated to one row
- **LSP**: one row per language server with its status (`starting`, `connected`, `broken`), or `LSPs will activate as files are read` when none has started
- **Modified files**: files changed in this Session with `+adds -dels`, at most 10 rows and then `+N more`; omitted when nothing changed
- at the bottom: `<cwd>:<branch>` (home shown as `~`, worktree name when in a managed worktree, branch omitted outside git), then a connection dot (green connected, yellow reconnecting, red disconnected) with `Cyber Code <version>`

MCP and LSP rows SHALL update from server events without polling, and the sidebar SHALL re-read their status on reconnect. `<leader>e` SHALL move focus into the sidebar: Up/Down move between rows, Enter or Space collapses or expands a section, Enter on a modified file opens its diff, `r` on a failed MCP server reconnects it, and Esc returns focus to the composer. Every value SHALL come from public API routes. A section whose source route is unavailable SHALL be omitted, never shown with placeholder values.

#### Scenario: Wide terminal shows the sidebar
- **WHEN** the terminal is 160 columns wide and `tui.sidebar` is `auto`
- **THEN** the sidebar is drawn at 48 columns with Context, MCP, LSP and the `cwd:branch` footer

#### Scenario: Failed MCP server
- **WHEN** MCP server `context7` fails to start with `spawn ENOENT`
- **THEN** its row shows a red dot, `Failed` and `spawn ENOENT`, and `r` on that row reconnects it

#### Scenario: No language servers yet
- **WHEN** no language server has started in the Location
- **THEN** the LSP section reads `LSPs will activate as files are read`

### Requirement: Responsive layout
(P1) The layout SHALL adapt to the terminal size, recomputed on every resize:
- 120 columns or more: transcript, plus the sidebar when it is enabled
- 60 to 119 columns: no docked sidebar; a 1-row header with the Session title and run state; `<leader>b` opens the sidebar as a drawer over the right side of the transcript, closed by Esc
- 40 to 59 columns: as above, and the status footer shows only context percent and cost
- below 40 columns or 10 rows: only the message `Terminal too small (40×10 minimum)`

No region SHALL draw outside its area at any size from 40×10 to 300×100. Truncated text SHALL end with `…`, and paths SHALL be shortened in the middle (`~/work/…/src/main.rs`).

#### Scenario: Shrinking hides the sidebar
- **WHEN** a 140-column terminal with the sidebar visible is resized to 100 columns
- **THEN** the sidebar is hidden, the header row appears, and `<leader>b` opens it as a drawer

### Requirement: Session recap line
(P1) The system SHALL show the latest Session recap (`session-runtime` → Session recap) as one block at the top of the transcript area: `※ recap: <text>` in italic `text_muted`, wrapped to at most 2 rows with `…`, and followed by a `border_active` rule. `tui.recap` SHALL accept `auto` (default) and `off`. In `auto` the TUI SHALL request a recap when a Session with at least one finished run is opened and its last activity is at least 30 minutes old, and when the terminal regains focus after at least 30 minutes away and there was activity since the last recap. `/recap` SHALL request one immediately, whatever the setting. The recap SHALL be hidden when the user submits the next prompt, and Esc SHALL dismiss it. A recap that fails SHALL show nothing except a toast for `/recap`.

#### Scenario: Recap on return
- **WHEN** the user resumes a Session last active 3 hours ago
- **THEN** the top of the transcript shows `※ recap:` with a summary of what was done and the next step

#### Scenario: Recap disabled
- **WHEN** `tui.recap` is `off` and the user resumes an old Session
- **THEN** no recap is requested and no model call is made

### Requirement: Settings panel
(P1) The system SHALL provide a full-screen Settings panel with the tabs Status, Config, Usage and Stats. `/settings` SHALL open the last used tab, and `/status`, `/config`, `/usage` and `/stats` SHALL open their own tab. `/cost` SHALL open the Usage tab scrolled to the Session section. The panel SHALL replace the transcript and composer while open, keep the recap block when one is shown, and draw the tab bar as `Settings  Status  Config  Usage  Stats`, with the active tab in `selection` (reverse video under `NO_COLOR`). Keys: Tab and Shift+Tab or Left and Right switch tabs, `1`–`4` jump to a tab, Up/Down, PageUp/PageDown, Home and End scroll, `r` refreshes, and Esc closes. The bottom row SHALL list the keys of the current tab. Each tab SHALL load its data when it is shown, show `Loading…` until the data arrives, and show a failed request as `Could not load <section>: <error> · r to retry` without closing the panel. A Drain keeps running while the panel is open, and a permission or question request SHALL close the panel and show its prompt.

#### Scenario: Direct tab command
- **WHEN** the user types `/stats`
- **THEN** the Settings panel opens with the Stats tab active

#### Scenario: Approval interrupts the panel
- **WHEN** a permission request arrives while the Usage tab is open
- **THEN** the panel closes and the permission prompt is shown

### Requirement: Settings Status tab
(P1) The Status tab SHALL show, one `label: value` row each:
- client version and server version, in `warning` with `restart the service` when they differ
- Session title and ID, Location directory, and git branch with a dirty marker
- agent, model with provider and variant, Mode, and context window use as a 30-column meter with tokens and percent
- the auto-compaction threshold
- a summary of MCP servers (connected out of configured, failed names) and of language servers

It SHALL include sandbox state and Cyber Account state only when the server reports them. It SHALL never show a placeholder for an unreported value.

#### Scenario: Version mismatch
- **WHEN** the TUI is 0.9.1 and the server reports 0.9.0
- **THEN** the version row shows both versions in the warning color with `restart the service`

### Requirement: Settings Config tab
(P1) The Config tab SHALL list TUI preferences that can be edited in place: `theme`, `theme_mode`, `sidebar`, `recap`, `reasoning`, `diff_style`, `timestamps`, `color` and `accessibility.reduced_motion`. Each row SHALL show the current value and its source (`default`, `global`, `project`, `session`). Enter or Space SHALL cycle a value or open its picker. A change SHALL apply immediately and be saved to `~/.local/state/cyber/kv.json`, and a later edit of `tui.jsonc` SHALL win over it. Below the preferences, the tab SHALL show effective core settings (`model`, `small_model`, `mode`, `default_agent`, `budgets`) read-only with their source file, when the server's `config` route is available. Otherwise it SHALL show `Core settings: use cyber config or edit cyber.jsonc`.

#### Scenario: Toggle the sidebar from Config
- **WHEN** the user selects `sidebar` and presses Enter until it reads `hide`
- **THEN** the sidebar disappears, and the value persists after a restart

### Requirement: Settings Usage tab
(P1) The Usage tab SHALL show the current Session tree (the Session and its descendants), using `GET /api/v1/usage?scope=session` and its breakdown:
- `Total cost` (`—` when every step is unpriced, `≥ $x · N unpriced calls` when some are)
- `Total duration (API)` and `Total duration (wall)`
- `Total code changes: N lines added, M lines removed`
- `Usage by model`: one row per model, `<model>: <input> input, <output> output, <cache read> cache read, <cache write> cache write (<cost>)`, sorted by cost and then by tokens, plus reasoning when it is non-zero
- the token class table and cache hit rate that `/cost` showed

Numbers SHALL use compact units (`5.3k`, `352.7k`, `1.2m`, `317.4m`, `4.6b`). Durations SHALL use `3h 21m 0s` or `5d 23h 31m`. Below the Session section the tab SHALL show the limit meters (Usage limit meters) and the contributing factors (Usage contributing factors).

#### Scenario: Two models in one Session
- **WHEN** a Session used a small model for titles and `claude-opus-5-5` for Turns
- **THEN** `Usage by model` lists both, with the larger cost first

#### Scenario: Unpriced local model
- **WHEN** every step of the Session used an unpriced Ollama model
- **THEN** `Total cost` shows `—`, not `$0.00`

### Requirement: Usage limit meters
(P1) The Usage tab SHALL show a 40-column meter with `N% used` for each Budget that is configured and applies to the Session (`budgets.session` and `budgets.daily`, per class: cost, tokens, Turns). Each meter SHALL be labelled with its scope and limit (`Session budget ($5.00)`, `Daily budget ($20.00)`), and the daily meter SHALL show its reset time in local time with the zone name. A meter SHALL use `meter_fill` and switch to `warning` at 80% and `error` at 100%. (P2) It SHALL also show each provider-reported usage window (`observability-costs` → Provider-reported limits) with the provider's label, percent used and reset time, and how old the reading is when older than 5 minutes. `d` and `w` SHALL switch the reported windows between their daily and weekly views, when the provider reports both. When no Budget is configured and no provider reports a limit, the section SHALL be omitted entirely. The system SHALL never estimate a provider limit from local usage.

#### Scenario: No limits anywhere
- **WHEN** no Budget is configured and the provider reports no usage window
- **THEN** the Usage tab has no limits section and no `% used` meter

#### Scenario: Daily budget at 85%
- **WHEN** `budgets.daily.max_cost_usd` is 20 and $17.00 was spent today
- **THEN** the `Daily budget ($20.00)` meter shows `85% used` in the warning color, with its reset time

### Requirement: Usage contributing factors
(P1) The Usage tab SHALL show `What's contributing to your usage?` from the server's usage insights (`observability-costs` → Usage insights). It SHALL show the window (`Last 24h`, or `Last 7 days` after `w`; `d` returns to 24 hours) and the notes `Approximate: Sessions recorded by this server only` and `These are independent characteristics of your usage, not a breakdown`. Each factor with a share of at least 5% SHALL be shown as a bold line `N% of your usage came from <factor>`, followed by one indented advice line in `text_muted`:
- subagent-heavy Sessions: `Each subagent runs its own requests. Spawn them deliberately, and give simple subagents a cheaper model.`
- Sessions active for 8+ hours: `These are often background or loop Sessions. Continuous usage adds up; make sure it is intended.`
- requests above 150k context: `Long contexts cost more even when cached. /compact mid-task, /new when switching tasks.`
- (P2) Workflow Runs: `A workflow fans a task out to many agents. Save workflows for big tasks.`

Then a `Subagents` table SHALL list each subagent group with its `% of usage`, highest first, at most 8 rows. The tab SHALL state whether shares are `% of cost` or `% of tokens`, as the server reports. When the window has no usage, the section SHALL read `No usage in the last 24h`.

#### Scenario: Subagent-heavy day
- **WHEN** the server reports that 62% of the last 24 hours of cost came from subagent-heavy Sessions
- **THEN** the tab shows `62% of your usage came from subagent-heavy sessions` with its advice line

#### Scenario: Week view
- **WHEN** the user presses `w` in the Usage tab
- **THEN** the factors and the Subagents table are reloaded for the last 7 days and the label reads `Last 7 days`

### Requirement: Settings Stats tab
(P1) The Stats tab SHALL render `GET /api/v1/usage/stats` in two subtabs, Overview and Models, switched with `v`. `p` SHALL cycle the period `All time · Last 7 days · Last 30 days`, with the active period in bold `accent`. `s` SHALL switch the scope between `All projects` and `This project`. Overview SHALL show:
- an activity heatmap: one column per week and one row per weekday, newest week on the right, `Mon`, `Wed` and `Fri` row labels, month labels above the first week of each month, cells colored `heat_0`–`heat_4` by daily token quartiles of active days (glyphs `·░▒▓█` under `NO_COLOR`), and a `Less ░ ▒ ▓ █ More` legend. It SHALL show as many recent weeks as fit, up to 53, and always the whole period
- `Favorite model`, `Total tokens`, `Sessions`, `Longest session`, `Active days: N/M`, `Longest streak`, `Current streak`, `Most active day`, and `Total cost` (with the same unpriced rules as the Usage tab), in two columns when the tab is at least 80 columns wide
- one `text_subtle` line `Input · Output · Reasoning · Cache read · Cache write` with compact numbers

Models SHALL list each model with total tokens, share as a 20-column bar and percent, cost, Turns and Sessions, sorted by tokens. Days SHALL be local days in the TUI's time zone, which it SHALL send to the server.

#### Scenario: Heatmap on a narrow panel
- **WHEN** the Stats tab is 70 columns wide with the period `All time`
- **THEN** the heatmap shows the most recent weeks that fit and no cell is clipped

#### Scenario: Last 7 days
- **WHEN** the user presses `p` once from `All time`
- **THEN** every metric is recomputed for the last 7 local days and `Last 7 days` is highlighted

## MODIFIED Requirements

### Requirement: Message and part rendering
(P0) The system SHALL render:
- assistant text as Markdown with syntax-highlighted code blocks (P1: as defined in Markdown rendering and Syntax-highlighted code blocks)
- reasoning collapsed by default; Ctrl+T toggles it per Session
- each tool call as a collapsible card with title, status (`pending`, `running`, `completed`, `failed`, `interrupted`), duration and summary
- file edits as diffs, in `unified` or `split` style (`tui.diff_style`, default `auto`: split when the terminal is at least 160 columns wide)

`/timestamps` SHALL toggle per-message timestamps. Tool output over 30 lines SHALL be collapsed to its last 10 lines until expanded. (P1) Diff lines SHALL use the `diff_*` theme tokens with `diff_added_bg` and `diff_removed_bg` backgrounds, and each finished run SHALL end with the Turn footer marker.

#### Scenario: Split diff on wide terminal
- **WHEN** an edit completes and the terminal is 200 columns wide with `diff_style` set to `auto`
- **THEN** the edit card renders a side-by-side diff

#### Scenario: Long bash output collapsed
- **WHEN** a bash tool returns 400 lines
- **THEN** the card shows the last 10 lines and `+390 lines (press o to expand)`

### Requirement: Status line
(P1) The system SHALL render a configurable status line as the last row of the screen, below the composer, from `tui.statusline.fields`. Available fields: `model`, `variant`, `agent`, `mode`, `context` (tokens and percent of the context window used, as `29.5K (22%)`), `cost` (`$0.00`, `—` when unpriced, `≥ $x` when partly unpriced), `branch`, `worktree`, `goal`, `loops`, `remote`, `cwd`. Default fields: `cwd, context, cost`, since the composer info line shows the agent, model and Mode, and the sidebar shows the branch. `cwd` SHALL be left-aligned and shortened in the middle to fit. The other fields SHALL be right-aligned, separated by ` · `, and followed by the key hint `<command_palette key> commands` (default `ctrl+p commands`), which reflects the configured binding. A toast SHALL replace the left side for 6 seconds. When `tui.statusline.command` is set, the system SHALL run that command at most once every 300 ms with Session state as JSON on stdin, and render the first line of its stdout in place of the fields.

#### Scenario: Custom status command
- **WHEN** `tui.statusline.command` is `~/.config/cyber/status.sh`
- **THEN** the script receives JSON including `session_id`, `model`, `mode`, `context_used_pct` and `cost_usd`, and its first stdout line is shown in the footer

#### Scenario: Context warning
- **WHEN** context usage exceeds 85%
- **THEN** the `context` field is rendered in the warning color

#### Scenario: Default footer
- **WHEN** a Session in `/Users/leo/work/app` has used 29,470 of 131,072 context tokens and $0.00
- **THEN** the footer shows `/Users/leo/work/app` on the left and `29.5K (22%) · $0.00  ctrl+p commands` on the right

### Requirement: Themes and appearance
(P0) The system SHALL ship at least 12 built-in themes, including `cyber`, `system`, `dracula`, `tokyonight`, `catppuccin`, `gruvbox`, `nord`, `one-dark`, `github-light` and `solarized-light`. It SHALL load custom themes from `themes/*.json` in global and project config directories. It SHALL follow the terminal's dark/light background when `tui.theme` is `system`. `/theme` SHALL preview themes live, and the choice SHALL be persisted to `~/.local/state/cyber/kv.json`. (P1) The theme SHALL reload on SIGUSR2 without a restart. (P1) Every theme SHALL resolve to the Theme tokens, and colors SHALL be rendered as Color capability and NO_COLOR defines.

#### Scenario: Custom theme override
- **WHEN** both global and project `themes/` contain `corp.json`
- **THEN** the project theme wins and is listed once as `corp`

### Requirement: Keybindings
(P0) The system SHALL merge `tui.keybinds` over the defaults, supporting:
- a leader key (default Ctrl+X) with a 2,000 ms timeout
- chord sequences such as `<leader>m`
- context-scoped bindings for `composer`, `messages`, `picker`, `agent_view` and `permission`, and (P1) `palette`, `sidebar` and `settings`
- disabling a binding with `"none"`

(P1) `tui.vim_mode: true` SHALL enable vim normal/insert modes in the composer, also toggled with `/vim`. (P1) The defaults SHALL include `command_palette` (Ctrl+P), `sidebar_toggle` (`<leader>b`), `sidebar_focus` (`<leader>e`) and `settings` (`<leader>,`). A binding SHALL never be shown in a hint, the help screen or the palette when it is disabled.

#### Scenario: Disable a default binding
- **WHEN** `tui.keybinds.composer.history_previous` is `"none"`
- **THEN** pressing Up in the composer moves the cursor and no longer recalls history

#### Scenario: Rebound palette updates the footer hint
- **WHEN** `tui.keybinds.command_palette` is `"ctrl+k"`
- **THEN** Ctrl+K opens the palette and the footer hint reads `ctrl+k commands`

### Requirement: Accessibility
(P1) The system SHALL provide `tui.accessibility.screen_reader: true` (or `CYBER_SCREEN_READER=1`). This mode disables spinners and animations, renders streaming text in whole-sentence chunks, labels every status with words rather than glyphs, and forces inline rendering. `tui.accessibility.reduced_motion` SHALL disable animations independently. Every theme SHALL meet a 4.5:1 contrast ratio for body text against its background, and 3:1 for `text_muted` and for focus indicators. Color SHALL never be the only carrier of meaning: every status dot, meter and heatmap SHALL also have a word, a percentage or a glyph level. Every panel, tab, sidebar row and palette entry SHALL be reachable and operable with the keyboard alone, and the focused element SHALL be marked with reverse video or a `›` marker. In screen-reader mode the sidebar SHALL not be drawn, the Turn footer marker SHALL be emitted as `Turn finished: agent <agent>, model <model>, <duration>`, and the Stats heatmap SHALL be replaced by a sentence such as `Active 265 of 340 days; most active May 20; current streak 14 days`.

#### Scenario: Screen reader mode
- **WHEN** screen-reader mode is on and a tool fails
- **THEN** the TUI emits the text line `Tool bash failed: <error>` instead of a red glyph-only card

#### Scenario: Heatmap as text
- **WHEN** screen-reader mode is on and the Stats tab opens
- **THEN** activity is described in one sentence and no heatmap grid is printed
