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

A theme JSON file (`themes/*.json`) SHALL have the form `{ "$schema"?, "defs"?: { name: "#rrggbb" }, "theme": { token: "#rrggbb" | defName | { "dark": value, "light": value } } }`. A token missing from a theme SHALL be derived from the theme's `text`, `background` and `accent` (for example `surface` = `background` blended 6% toward `text`, `heat_1..4` = `accent` at 25/50/75/100% over `background`). A theme with `{dark, light}` values SHALL render the variant that matches the terminal background. The built-in `cyber` theme SHALL define both variants. `tui.theme_mode` SHALL accept `auto` (default), `dark` and `light`. In `auto` the system SHALL detect the background with an OSC 11 query (100 ms timeout), then `COLORFGBG`, and SHALL use `dark` when neither answers. The query SHALL never delay a frame or leak into input: the first frame uses `COLORFGBG` or `dark`, the reply's bytes SHALL be consumed before key handling, and a reply that arrives within 2 seconds SHALL re-resolve the theme. The query is sent only on Unix terminals; Windows uses `COLORFGBG` or `dark`.

#### Scenario: Light terminal picks the light variant
- **WHEN** the theme is `cyber`, `tui.theme_mode` is `auto`, and the terminal answers OSC 11 with a white background
- **THEN** the TUI renders the `cyber` light variant, with body text meeting 4.5:1 contrast against the background

#### Scenario: Partial custom theme
- **WHEN** `themes/corp.json` defines only `text`, `background` and `accent`
- **THEN** the theme loads, and `surface`, `text_muted` and `heat_1..4` are derived instead of failing

### Requirement: Color capability and NO_COLOR
(P1) The system SHALL render colors at the terminal's capability: 24-bit when `COLORTERM` is `truecolor` or `24bit` (or `WT_SESSION` is set, for Windows Terminal), else the nearest xterm-256 color when `TERM` contains `256color`, else the nearest of the 16 ANSI colors. `tui.color` SHALL override detection with `auto` (default), `truecolor`, `256`, `16` or `none`. When the `NO_COLOR` environment variable is set to a non-empty value, or `tui.color` is `none`, the system SHALL emit no foreground or background colors. It SHALL convey state only with text attributes (bold, italic, underline, reverse), glyphs and words: selection and focus in reverse video, status dots paired with a status word, and heatmap levels as glyph density (`·░▒▓█`). `NO_COLOR` SHALL take precedence over `tui.color`.

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
- links as their text, with the URL as an OSC 8 hyperlink when the terminal supports it and otherwise appended in `text_subtle`. Support SHALL be detected from the environment (`TERM_PROGRAM`, `WT_SESSION`, `KITTY_WINDOW_ID`, `VTE_VERSION` ≥ 5000, or a `TERM` naming kitty, foot, Alacritty, Ghostty or WezTerm) and SHALL be off inside tmux or screen. Only `http`, `https` and `mailto` URLs of at most 2,048 printable ASCII characters without spaces SHALL be written inside an OSC 8 sequence, and only over cells that still show the link text; any other link keeps its URL appended as text
- thematic breaks as a `md_rule` line across the transcript width

Wrapped list items and quotes SHALL keep their hanging indent. While a message streams, the system SHALL re-parse only from the start of the last unfinished block, so earlier blocks never reflow. Raw HTML SHALL be shown as literal text.

#### Scenario: Numbered list with bold terms
- **WHEN** the assistant returns `1. **Memoization:** Results are cached`
- **THEN** the line shows `1. ` followed by `Memoization:` in bold and the rest in `text`, with no asterisks

#### Scenario: Wide table on a narrow terminal
- **WHEN** a 6-column table needs 140 columns and the transcript is 80 columns wide
- **THEN** each row is rendered as `header: value` lines instead of a clipped table

### Requirement: Syntax-highlighted code blocks
(P1) The system SHALL highlight fenced code blocks, and code in tool cards for `read` and `write`, with a bundled pure-Rust highlighter mapped onto the `syntax_*` tokens. The language SHALL come from the fence info string (aliases such as `ts`, `py`, `sh` and `rs` resolved), else from the file extension for tool output. Unknown languages SHALL render in `md_code` without failing. A `read` card SHALL show its line numbers in `text_subtle` beside the highlighted code, and a `write` card SHALL show the written content under the tool's result line. A code block SHALL render with a `surface` background, a 2-column left padding, and a dim language label on its first row. Highlighting SHALL run only for finished blocks; the streaming tail of an open block SHALL render plain until the closing fence arrives. Blocks longer than 2,000 lines or lines longer than 1,000 characters SHALL render plain. Highlight results SHALL be cached per block and theme. Code SHALL never be re-wrapped mid-token for highlighting; long lines wrap at the transcript width with a `↪` continuation marker in `text_subtle`. The `<leader>y` binding (default Ctrl+X y) SHALL select the last code block, marking its label row with `›` in the selection style; Up and Down SHALL move the selection between blocks, Esc SHALL leave it, and any other key SHALL leave it and act as usual. Pressing `y` (or Enter) on a selected code block, or using the command palette's "Copy last code block", SHALL copy the code without padding or markers through a native clipboard tool (`pbcopy`, `wl-copy`, `xclip`, `xsel`), using OSC 52 when none is available, including over SSH and on Windows. The OSC 52 payload SHALL be base64 and at most 100,000 bytes. Every copy (code blocks and explorer paths) SHALL hold only what the screen drew: control characters other than newline and tab, and the bidi controls U+202A–U+202E and U+2066–U+2069, SHALL be removed, so an ESC or a bracketed-paste end marker in model output or a file name never reaches the clipboard.

#### Scenario: Python block highlighted
- **WHEN** the assistant returns a fenced `python` block containing `# comment` and `for i in range(10):`
- **THEN** the comment renders in `syntax_comment` italic and `for` in `syntax_keyword`

#### Scenario: Code block copied over SSH
- **WHEN** `SSH_CONNECTION` is set and the user presses Ctrl+X y, then `y`
- **THEN** the code is sent as one OSC 52 sequence and a toast reads `Copied <n> lines (OSC 52)`

#### Scenario: Unsafe link URL
- **WHEN** the terminal supports OSC 8 and the assistant writes `[x](javascript:alert(1))`
- **THEN** no OSC 8 sequence is written and the row reads `x (javascript:alert(1))`

#### Scenario: Streaming block stays plain until closed
- **WHEN** a fenced block is still streaming
- **THEN** its lines render in `md_code` and are highlighted once the closing fence arrives, with no change to the row count

### Requirement: Turn footer marker
(P1) After the last assistant message of each finished run (the Turns that answer one promoted user input), the system SHALL render one marker line: `▣ <Agent> · <model> · <duration>`. `<Agent>` is the agent's name with its first letter upper-cased, colored with the agent's configured `color` (the `color` field of `GET /api/v1/agents`, used only when it is `#rrggbb` or an ANSI color name) or else `accent`. `<model>` is the display name from `GET /api/v1/models`, falling back to the model ID. `<duration>` is the wall time from the first Turn's `started_ms` to the last Turn's `ended_ms`. Durations SHALL be formatted as `850ms`, `11.3s`, `4m 05s` or `1h 02m`. A run is interrupted when its last step failed with kind `interrupted` or a tool call of that step was interrupted, and failed when its last step failed otherwise. An interrupted run SHALL end with `· interrupted` in `warning`, and a failed run with `· failed` in `error`. The marker SHALL be built only from durable message data, so it is the same after a reload, and SHALL not be shown while the run is still active. `/timestamps` SHALL add the local end time (`14:02`) to the marker.

#### Scenario: Marker after a finished answer
- **WHEN** the `build` agent on model `amini-coder` finishes a run whose Turns span 11.3 seconds
- **THEN** the transcript shows `▣ Build · amini-coder · 11.3s` below the answer

#### Scenario: Interrupted run
- **WHEN** the user presses Esc 4.2 seconds into a run
- **THEN** the marker reads `▣ Build · amini-coder · 4.2s · interrupted` in the warning color

### Requirement: Composer appearance
(P1) The composer SHALL be drawn as a `surface` block with a 1-column left accent bar `┃` and no other border. The accent bar color SHALL show the input mode: the current agent's color for prompts, `warning` for `!` shell input, `info` for `/` commands and `secondary` for `#` memory input. The last row of the composer SHALL be an info line, `<Agent> · <model> <provider name>`, followed by the Mode label (and the variant when the model has variants), in `text_muted`; for `!` input it SHALL read `shell` and for `/` input `command`. While a Drain runs, the progress line above the composer (Nyan progress indicator) SHALL carry the activity and `esc interrupt`; with `tui.spinner` set to `off` the info line SHALL end with `esc interrupt` instead. An empty composer SHALL show the placeholder `Ask anything… "/" commands · "@" files · "!" shell` in `text_subtle`. The text area SHALL grow from 1 to 8 rows and then scroll. Pending paste and image chips SHALL render in `accent` on `selection`.

#### Scenario: Shell mode recolors the bar
- **WHEN** the user types `!` as the first character
- **THEN** the accent bar turns to the warning color and the info line shows `shell`

#### Scenario: Running state above the composer
- **WHEN** a Drain is running
- **THEN** the progress line above the composer shows the indicator and `esc interrupt`, and the info line ends with `esc interrupt` only when `tui.spinner` is `off`

### Requirement: Command palette
(P1) The system SHALL open a command palette with the `command_palette` keybinding (default Ctrl+P), and with `/` followed by Tab in an empty composer. It SHALL list every built-in TUI command, custom command, skill and MCP prompt, plus UI actions (toggle sidebar, open settings tabs, switch theme, copy last code block, toggle reasoning). Each row SHALL show the name, a one-line description and the current key binding. A command that declares an argument hint SHALL be typed into the composer as `/name ` instead of run. Names and descriptions from commands, skills and MCP prompts SHALL be shown on one line without control characters. Typing SHALL filter by fuzzy match, Enter SHALL run the selected entry, and Esc SHALL close the palette without side effects.

#### Scenario: Open settings from the palette
- **WHEN** the user presses Ctrl+P, types `usage` and presses Enter
- **THEN** the Settings panel opens on the Usage tab

### Requirement: Welcome screen
(P1) When a Session has no messages, the transcript area SHALL show a centered welcome screen. From top to bottom it SHALL contain:
- the `CYBER CODE` ASCII-art logo in box-drawing block letters, exactly as in `assets/logo-wide.txt` (6 rows, 78 columns) when the transcript area is at least 82 columns wide, `assets/logo-compact.txt` (`CYBER` above `CODE`, 41 columns) from 45 to 81 columns, and the bold text `Cyber Code` below 45 columns. Full-block cells SHALL be colored with a horizontal gradient from the `accent` token to the `secondary` token. Shadow (box-drawing) cells SHALL use the same gradient dimmed toward `background`. With 16 colors the logo SHALL use `accent` only, and under `NO_COLOR` or monochrome it SHALL render the glyphs with no color attributes
- a `text_muted` line with the version and the tagline `local-first coding agent`
- a panel (`background_panel`) with agent, model and variant, Mode, `cwd:branch`, the MCP summary (a status dot and `N connected`, or `N failed` in `error`) and the active LSP servers, followed by a `Tips` block of key hints generated from the binding table, so rebound keys show their current binding
The welcome screen SHALL disappear as soon as the first message exists and SHALL never be part of the transcript, a share or an export. `tui.welcome` (`full` default, `compact`, `off`) SHALL select the full screen, the panel without the logo, or nothing. Screen readers SHALL get the single line `Cyber Code <version>` followed by the panel as text, with no logo glyphs. When the terminal is shorter than the logo plus the panel plus the composer, the logo SHALL be dropped first and then the tips; tips SHALL be shown only when they fit below the panel. The panel SHALL use `surface`, and the tips SHALL list the current keys of `command_palette`, `sidebar_toggle`, `model_list`, `session_list` and `help`, leaving out disabled ones.

#### Scenario: Wide terminal
- **WHEN** a new Session opens at 120×34
- **THEN** the wide logo, version line, panel and tips are centered above the composer

#### Scenario: Narrow terminal
- **WHEN** a new Session opens at 60×30
- **THEN** the compact two-word logo is shown, and no row is clipped

#### Scenario: Welcome off
- **WHEN** `tui.welcome` is `off`
- **THEN** an empty Session shows only the composer and the footer

#### Scenario: First prompt
- **WHEN** the user sends the first prompt
- **THEN** the welcome screen is gone and the transcript starts at the top

### Requirement: Session sidebar
(P1) The system SHALL provide a right sidebar, toggled by `<leader>b` and `/sidebar`, with `tui.sidebar` set to `auto` (default), `show` or `hide`. In `auto` and `show` it SHALL be docked when the terminal is at least 120 columns wide; `hide` SHALL start hidden, and the toggle SHALL flip the docked state for the rest of the run. Its width SHALL be 30% of the terminal, clamped to 32..48 columns. From top to bottom it SHALL show:
- the Session title, and the parent title for child Sessions
- **Context**: used tokens, percent of the context window used (`warning` above 85%), and cost spent (`—` when unpriced, `≥ $x` when partly unpriced)
- **MCP**: a collapsible list (`▼`/`▶`), with one row per configured server: a status dot and the status word (`Connected`, `Connecting`, `Failed`, `Needs auth`, `Disabled`); a failed server shows its error, truncated to one row
- **LSP**: one row per language server with its status (`starting`, `connected`, `broken`), or `LSPs will activate as files are read` when none has started
- **Modified files**: files changed in this Session with `+adds -dels` (from `GET .../diff` without `message_id`, the diff from the Session's first pre-Turn snapshot to its latest post-Turn snapshot), at most 10 rows and then `+N more`; omitted when nothing changed
- at the bottom: `<cwd>:<branch>` (home shown as `~`, worktree name when in a managed worktree, branch omitted outside git), then a connection dot (green connected, yellow reconnecting, red disconnected) with `Cyber Code <version>`

MCP rows SHALL update from `mcp.status.changed.1` and `mcp.tools.changed.1`, Modified files from `session.diff.1`, and LSP and the branch SHALL be re-read when the Session goes idle (language servers start as files are read), all without polling; every section SHALL be re-read when the event stream reconnects. A dropped event stream SHALL be reopened every 2 seconds, with the connection dot yellow while it retries and red after a failed attempt. `<leader>e` SHALL move focus into the sidebar, opening the drawer below 120 columns: Up/Down (and `j`/`k`) move between rows, Enter or Space collapses or expands MCP or Modified files, Enter on a modified file opens its diff, `r` on a failed or `needs_auth` MCP server reconnects it through `POST /api/v1/mcp/{name}/reconnect`, and Esc returns focus to the composer (closing the drawer). These keys SHALL be the `sidebar` context's bindings `sidebar_next`, `sidebar_previous`, `sidebar_select`, `sidebar_reconnect` and `sidebar_close`. The reconnect route SHALL accept a server that settled `failed` only when it retained no native owner, so a failure with unverified ownership stays refused with `409`. All server-provided names, errors, paths and patches SHALL be shown on one line (patches per line) without control characters. Every value SHALL come from public API routes. A section whose source route is unavailable SHALL be omitted, never shown with placeholder values.

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
- 60 to 119 columns: no docked sidebar; a 1-row header with the Session title and run state (`idle`, or a spinner and `running`); `<leader>b` opens the sidebar as a drawer over the right side of the transcript, which takes focus and is closed by Esc
- 40 to 59 columns: as above, and the status footer shows only context percent and cost
- below 40 columns or 10 rows: only the message `Terminal too small (40×10 minimum)`

No region SHALL draw outside its area at any size from 40×10 to 300×100. Truncated text SHALL end with `…`, and paths SHALL be shortened in the middle (`~/work/…/src/main.rs`).

#### Scenario: Shrinking hides the sidebar
- **WHEN** a 140-column terminal with the sidebar visible is resized to 100 columns
- **THEN** the sidebar is hidden, the header row appears, and `<leader>b` opens it as a drawer

### Requirement: Session recap line
(P1) The system SHALL show the latest Session recap (`session-runtime` → Session recap) as one block at the top of the transcript area: `※ recap: <text>` in italic `text_muted`, wrapped to at most 2 rows with `…`, and followed by a `border_active` rule. `tui.recap` SHALL accept `auto` (default) and `off`. In `auto` the TUI SHALL request a recap when a Session with at least one finished run is opened and its last activity is at least 30 minutes old, and when the terminal regains focus after at least 30 minutes away and there was activity since the last recap. `/recap` SHALL request one immediately, whatever the setting. The recap SHALL be hidden when the user submits the next prompt, and Esc SHALL dismiss it (while a run is in progress Esc still interrupts it first). A recap is asked for at most once per opened Session; the server's `through_seq` cache answers repeated requests without a model call. A recap that fails SHALL show nothing except a toast for `/recap`.

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
(P1) The Config tab SHALL list TUI preferences that can be edited in place: `theme`, `theme_mode`, `sidebar`, `recap`, `spinner`, `reasoning`, `diff_style`, `timestamps`, `color` and `accessibility.reduced_motion`. Each row SHALL show the current value and its source (`default`, `global`, `project`, `saved` for `kv.json`, or `session` for a toggle made in this run only, such as Ctrl+T or `/timestamps`). Enter or Space SHALL cycle a value or open its picker. A change SHALL apply immediately and be saved to `~/.local/state/cyber/kv.json` with the time it was saved, and a `tui.jsonc` written after that time SHALL win over it. Below the preferences, the tab SHALL show effective core settings (`model`, `small_model`, `mode`, `default_agent`, `budgets`) read-only with their source file, when the server's `config` route is available. Otherwise it SHALL show `Core settings: use cyber config or edit cyber.jsonc`.

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
(P1) The Usage tab SHALL show a 40-column meter with `N% used` for each Budget that is configured and applies to the Session (`budgets.session` and `budgets.daily`, per class: cost, tokens, Turns). Each meter SHALL be labelled with its scope and limit (`Session budget ($5.00)`, `Daily budget ($20.00)`), and the daily meter SHALL show its reset time in local time with the zone name. A meter SHALL use `meter_fill` and switch to `warning` at 80% and `error` at 100%. (P2) It SHALL also show each provider-reported usage window (`observability-costs` → Provider-reported limits) below the Budget meters, as a meter labelled `<provider> · <label>` (`test · 5-hour limit`) with the same colors, then `Resets <time> (<zone>)` in local time (with the date, `Oct 23, 4:00am`, when the reset is a day or more away) and `Reading from <age> ago` when the reading is older than 5 minutes. `d` and `w` SHALL switch the reported windows between their daily and weekly views (the same keys that switch the contributing factors' window), when the provider reports both; a provider that reports only one kind shows its windows in both views. A server without the route (`404`/`503`) counts as reporting nothing, and a malformed reply is refused whole. When no Budget is configured and no provider reports a limit, the section SHALL be omitted entirely. The system SHALL never estimate a provider limit from local usage.

#### Scenario: No limits anywhere
- **WHEN** no Budget is configured and the provider reports no usage window
- **THEN** the Usage tab has no limits section and no `% used` meter

#### Scenario: Provider windows by day and week
- **WHEN** the provider reported its 5-hour window at 85% and its weekly window at 30%, 12 minutes ago
- **THEN** the tab shows `test · 5-hour limit` at `85% used` in the warning color with `Resets 6:00am (UTC) · Reading from 12m ago`, and after `w` only `test · Weekly limit` at `30% used`

#### Scenario: Daily budget at 85%
- **WHEN** `budgets.daily.max_cost_usd` is 20 and $17.00 was spent today
- **THEN** the `Daily budget ($20.00)` meter shows `85% used` in the warning color, with its reset time

### Requirement: Usage contributing factors
(P1) The Usage tab SHALL show `What's contributing to your usage?` from the server's usage insights (`observability-costs` → Usage insights). It SHALL show the window (`Last 24h`, or `Last 7 days` after `w`; `d` returns to 24 hours) and the notes `Approximate: Sessions recorded by this server only` and `These are independent characteristics of your usage, not a breakdown`. Each factor with a share of at least 5% SHALL be shown as a bold line `N% of your usage came from <factor>`, followed by one indented advice line in `text_muted`:
- subagent-heavy Sessions: `Each subagent runs its own requests. Spawn them deliberately, and give simple subagents a cheaper model.`
- Sessions active for 8+ hours: `These are often background or loop Sessions. Continuous usage adds up; make sure it is intended.`
- requests above 150k context: `Long contexts cost more even when cached. /compact mid-task, /new when switching tasks.`
- (P2) Workflow Runs: `A workflow fans a task out to many agents. Save workflows for big tasks.`

Then a `Subagents` table SHALL list each subagent group with its share, highest first, at most 8 rows. The factors cover every Session the server stored (not only the open one), and `d`/`w` reload only this section. Factor names the TUI does not know are not shown, and a malformed reply is refused whole. The tab SHALL state whether shares are `% of cost` or `% of tokens`, as the server reports. When the window has no usage, the section SHALL read `No usage in the last 24h`.

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
- one `accent` comparison line relating the period's input plus output tokens to a well-known book, for example `Your input and output are ~7718x the tokens in Brave New World`. It SHALL pick the largest book from a small embedded table (title and approximate token count) that the total exceeds, fall back to a smaller-scale phrase when the total is below the smallest book, and be omitted when the period has no usage

Models SHALL list each model with total tokens, share as a 20-column bar and percent, cost, Turns and Sessions, sorted by tokens. Days SHALL be local days in the TUI's time zone, which it SHALL send to the server. The heatmap rows SHALL run Sunday to Saturday, and level 0 (a day without Turns) SHALL be `·`. A reply that fails validation (unparsable dates, token classes that do not add up, a share outside 0–1, a negative or non-finite cost) SHALL be refused whole, and model names SHALL be shown with control characters removed. The bottom row SHALL list `p period · v view · s scope · r refresh`.

#### Scenario: Book comparison line
- **WHEN** the selected period has 600 million input plus output tokens and the embedded table lists Brave New World at about 77,700 tokens
- **THEN** Overview shows `Your input and output are ~7718x the tokens in Brave New World`, or the same line for a larger book from the table that the total exceeds

#### Scenario: Heatmap on a narrow panel
- **WHEN** the Stats tab is 70 columns wide with the period `All time`
- **THEN** the heatmap shows the most recent weeks that fit and no cell is clipped

#### Scenario: Last 7 days
- **WHEN** the user presses `p` once from `All time`
- **THEN** every metric is recomputed for the last 7 local days and `Last 7 days` is highlighted

### Requirement: File explorer
(P1) The TUI SHALL provide a file explorer, opened with `/files` or the `file_explorer` binding (default `<leader>o`, that is `ctrl+x o`, because `<leader>e` is `sidebar_focus`) and closed with Esc. It SHALL show the Location's project as a tree in a pane on `surface_alt`: a header bar (`surface`) with `EXPLORER`, the project name and `:branch`, and at the right `✓ clean` in `success` or `N changed` in `warning`; a filter line with a `⌕` placeholder; the tree; and a summary line (`N shown · M hidden · .gitignore`). Root entries SHALL have no guide; each deeper level SHALL add a `│ ` indent guide in `text_subtle`, followed by the fold marker (`▸` collapsed, `▾` expanded). Directory names SHALL be bold in `file_dir` with a trailing `/`, an expanded directory SHALL show its loaded child count, and file names SHALL be colored by kind from their name: `file_doc` (Markdown and text), `file_config` (TOML, JSON, YAML, `justfile` and similar), `file_code` (source files) and `file_dim` (hidden, ignored, lock and generated files). A right-aligned column SHALL show the git status letter and the compact size (`1.2K`) in `text_muted`. The selected row SHALL have an `accent` bar and the `selection` background (reverse video without color). `tui.explorer.icons` (`none` default, `nerd`) SHALL add Nerd Font glyphs before names. The footer SHALL show the explorer keys as chips (the key on a `text_muted` background, then its label), generated from the binding table. Directories SHALL sort before files and SHALL be loaded lazily, one level per expansion, through `GET /api/v1/files`. Each row SHALL show the git status letter (`M` in `warning`, `A` and `?` in `success`, `D` in `error`) from the same VCS data as the sidebar, and a directory SHALL show `M` when any descendant changed. `.gitignore` and `.cyberignore` SHALL apply by default; `.` SHALL toggle hidden and ignored entries, and the footer SHALL count what is hidden. `/` SHALL filter by fuzzy path match across already-loaded and server-searched entries, and `g` SHALL show only changed files.

Enter on a file SHALL open a preview pane to the right, or replace the tree when the terminal is narrower than 100 columns. The preview SHALL have its own header bar with a breadcrumb (`docs › README.md`), the language, line count and size, and the `+N −M` diff summary. A vertical divider SHALL separate it from the tree. Markdown files SHALL be rendered through the transcript's Markdown renderer by default (no OSC 8 links are written from the preview), and `m` SHALL toggle the raw source, shown by a `rendered`/`raw` chip in the header. Source (code, or raw Markdown with markup scopes colored, never one color) SHALL have line numbers, a gutter mark on changed lines and a `│` separator, with syntax highlighting through the same highlighter and theme tokens as code blocks; long lines SHALL soft-wrap with a hanging indent under the gutter. A scrollbar thumb SHALL show the position when the preview is longer than the pane. `←` SHALL fold the selected directory or move to its parent, and `→` SHALL unfold it. `@` SHALL insert the selected path as a file mention in the composer, relative to the Session directory (absolute when the file lies outside it, as when the Location is below the project root), `y` SHALL copy the project-relative path, and `r` SHALL refresh. The explorer SHALL refresh changed rows on file-change and VCS events without collapsing the user's expansion state.

The explorer SHALL never bypass permissions. Listing SHALL be evaluated as `list` and preview as `read` against the Session's rules for that path. A file whose `read` decision is `ask` SHALL appear dimmed with `⊘ ask to read`, and opening its preview SHALL raise the normal permission request. A file whose decision is `deny` SHALL appear dimmed with `⊘ denied`, with no content. Preview SHALL be capped at 1 MiB and 5,000 lines, with a truncation notice. Binary files SHALL show `binary · <size>`, and symlinks SHALL show their target, never followed outside the project. Displayed file names and content SHALL be sanitized like other untrusted output, so no terminal escape sequence reaches the terminal. Screen readers SHALL get each row as `<name>, directory|file, <status>, level <n>`.

#### Scenario: Browse and preview
- **WHEN** the user opens the explorer, expands `crates/cyber-tui/src` and presses Enter on `markdown.rs`
- **THEN** the preview shows the file with line numbers, Rust highlighting and the changed-line gutter

#### Scenario: Secret file is not leaked
- **WHEN** `.env` matches the default `read` ask rule and the user selects it
- **THEN** the row shows `⊘ ask to read`, no content is fetched until the user approves the permission request, and a denial shows nothing

#### Scenario: Mention from the explorer
- **WHEN** the user presses `@` on `crates/cyber-tui/src/app.rs`
- **THEN** the composer contains `@crates/cyber-tui/src/app.rs` and the explorer closes

#### Scenario: Narrow terminal
- **WHEN** the terminal is 80 columns wide
- **THEN** the tree fills the width, and Enter replaces it with the preview, which Esc returns from

#### Scenario: Markdown rendered, then raw
- **WHEN** the user opens `docs/README.md` in the preview and presses `m`
- **THEN** the preview first shows the rendered document under a `rendered` chip, and after `m` shows the source with line numbers and heading, link and code colors

#### Scenario: Kinds are colored in the tree
- **WHEN** a directory lists `src/`, `README.md`, `Cargo.toml`, `main.rs` and `Cargo.lock`
- **THEN** `src/` is bold in `file_dir`, the files are in `file_doc`, `file_config`, `file_code` and `file_dim`, and root rows start without a guide

### Requirement: Theme switcher
(P1) `/theme`, the palette action `Theme…` and the Config tab's theme row SHALL open the theme switcher, a two-pane overlay. The left pane SHALL list built-in and custom themes grouped under `DARK` and `LIGHT` by the theme's resolved background luminance. Each row SHALL show the theme name, `current` on the active theme, `custom` on themes loaded from `themes/*.json`, and a swatch of its `accent`, `secondary`, `success`, `warning` and `error` tokens drawn on the theme's own background. Tab SHALL cycle the filter `All · Dark · Light`, and typing SHALL fuzzy-search names. The right pane SHALL show a live preview rendered with the highlighted theme's tokens: the welcome logo gradient, a Markdown heading, a list and a link, a highlighted code block, an added and a removed diff line, success, warning and error status glyphs, a Turn footer marker, and the measured contrast of `text` and `text_muted` against `background` with the WCAG level. Moving the selection SHALL apply the theme to the whole TUI immediately. Enter SHALL keep it and persist it to `kv.json`, `p` (with an empty search, since typing searches) SHALL write `theme` to the `.cyber/tui.jsonc` of the project root, refusing a file with comments rather than rewriting it, and Esc SHALL restore the theme that was active when the switcher opened. The `theme_cycle` binding (default `ctrl+x t`) SHALL switch to the next theme in the current filter without opening the switcher, keep it in `kv.json`, and SHALL announce `Theme: <name>` in the status footer for 2 seconds; `theme_list`, which opens the switcher, moves to `<leader>s`. A permission or question request that replaces the switcher SHALL restore the theme it opened with. Under `NO_COLOR` the switcher SHALL list themes and explain that colors are disabled, with no swatches. A theme whose `text` contrast is below 4.5:1 SHALL show `low contrast` in `warning` in the preview.

The built-in themes SHALL include the existing set plus `cyber-light`, `synthwave-84`, `rose-pine`, `kanagawa`, `ayu-mirage` and `matrix`, each defining every token or deriving it by the documented derivation rules, and each passing the contrast test.

#### Scenario: Live preview and revert
- **WHEN** the user opens `/theme`, moves to `synthwave-84` and presses Esc
- **THEN** the TUI showed `synthwave-84` while it was selected and returns to the previous theme, and nothing is persisted

#### Scenario: Apply and persist
- **WHEN** the user selects `rose-pine`, presses Enter and restarts the TUI
- **THEN** the TUI starts with `rose-pine`

#### Scenario: Cycle without the switcher
- **WHEN** the user presses `ctrl+x t` twice with the filter `Dark`
- **THEN** the theme moves two dark themes forward and the footer shows `Theme: <name>`

### Requirement: Nyan progress indicator
(P1) While the Session is working (a Turn, compaction, a running workflow the user started, or a long tool call), the progress line above the composer SHALL show the activity indicator selected by `tui.spinner`: `nyan` (default), `nyan-tall`, `braille`, `dots` or `off`.

`nyan` SHALL be one row: a rainbow trail of `▀` cells, each with two adjacent bands of the six-band rainbow (red, orange, yellow, green, blue, purple) as foreground and background, followed by the cat `~[,,_,,]:3` in a pink accent, then the activity label, the elapsed time and the interrupt hint. `nyan-tall` SHALL use 4 rows: 3 rows (6 half-rows, one band each) with every other pair of columns offset by half a row into the fourth to form the wave, and the 4-row cat art, and SHALL fall back to `nyan` when the terminal has fewer than 30 rows.

When the progress is unknown, the trail SHALL fill the available width and its colors SHALL scroll by one cell per frame. Compaction SHALL show the label `compacting` in `warning` with its elapsed time, as unknown progress while the server reports no fraction. When progress is known (workflow agents done over total, or byte progress of a download or upload), the trail length SHALL be proportional to the fraction, the remainder SHALL be drawn as `·` in `text_subtle`, and the label SHALL show the count and percent.

Frames SHALL advance at no more than 12 per second, only while working. Rendering SHALL be skipped when the terminal is unfocused or the indicator is off screen. With 16 colors the bands SHALL map to the nearest ANSI colors. Under `NO_COLOR` the trail SHALL be `=-` text, and with reduced motion the indicator SHALL be the static `[,,_,,]:3` with no animation. Screen readers SHALL receive `Working, <n> seconds` at most every 10 seconds and the label changes, never the trail glyphs.

#### Scenario: Unknown progress
- **WHEN** a Turn has been running for 12 seconds with `tui.spinner` at the default
- **THEN** the progress line shows the scrolling rainbow trail, the cat, `Thinking · 12.4s` and the interrupt hint

#### Scenario: Workflow progress
- **WHEN** a workflow run started from this Session has 7 of 10 agents done
- **THEN** the trail covers 70% of its width and the label reads `<name> 7/10 agents · 70%`

#### Scenario: Reduced motion
- **WHEN** reduced motion is on
- **THEN** the indicator is static and no animation frames are scheduled

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
(P0) The system SHALL ship at least 12 built-in themes, including `cyber`, `system`, `dracula`, `tokyonight`, `catppuccin`, `gruvbox`, `nord`, `one-dark`, `github-light` and `solarized-light`. It SHALL load custom themes from `themes/*.json` in global and project config directories. Since a project's files may be untrusted, a theme file over 64 KiB, a `tui.jsonc` over 256 KiB, and either one when it is not a regular file (a FIFO, or a link to a device) SHALL be skipped with a warning, without reading past the limit or waiting on the file. It SHALL follow the terminal's dark/light background when `tui.theme` is `system`. `/theme` SHALL preview themes live, and the choice SHALL be persisted to `~/.local/state/cyber/kv.json`. (P1) The theme SHALL reload on SIGUSR2 without a restart. (P1) Every theme SHALL resolve to the Theme tokens, and colors SHALL be rendered as Color capability and NO_COLOR defines.

#### Scenario: Custom theme override
- **WHEN** both global and project `themes/` contain `corp.json`
- **THEN** the project theme wins and is listed once as `corp`

### Requirement: Keybindings
(P0) The system SHALL merge `tui.keybinds` over the defaults, supporting:
- a leader key (default Ctrl+X) with a 2,000 ms timeout
- chord sequences such as `<leader>m`
- context-scoped bindings for `composer` (`history_previous`, `history_next`, `newline`, `editor`), and (P1) `palette`, `sidebar`, `settings` and `explorer`; `messages`, `picker`, `agent_view` and `permission` have no rebindable actions yet and are reported as unknown contexts
- disabling a binding with `"none"`

(P1) `tui.vim_mode: true` SHALL enable vim normal/insert modes in the composer, also toggled with `/vim`. (P1) The defaults SHALL include `command_palette` (Ctrl+P), `sidebar_toggle` (`<leader>b`), `sidebar_focus` (`<leader>e`), `settings` (`<leader>,`), `file_explorer` (`<leader>o`), `theme_list` (`<leader>s`) and `theme_cycle` (`<leader>t`). Top-level `tui.keybinds` entries SHALL bind global actions, and an object named after a context SHALL bind that context's actions. A binding SHALL never be shown in a hint, the help screen or the palette when it is disabled. A value that does not parse, an unknown action or an unknown context SHALL be reported and leave the default in place.

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
