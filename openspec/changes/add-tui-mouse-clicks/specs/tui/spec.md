## MODIFIED Requirements

### Requirement: Rendering modes and terminal integration
(P0) The system SHALL support two rendering modes, selected by `tui.render` (default `fullscreen`, or `--inline`):
- **fullscreen**: alternate screen with mouse support and stable memory. The TUI SHALL capture the mouse unless `tui.mouse` is `off`: a wheel notch SHALL scroll the transcript by 3 lines, or move Up/Down in a panel that scrolls or lists, and prompts and confirmations SHALL ignore the wheel. (P1) The wheel SHALL act on the pane under the pointer: on the main screen it SHALL do nothing over the sidebar and scroll the transcript elsewhere, and in the file explorer it SHALL follow File explorer. (P1) Left clicks SHALL act as Mouse clicks defines. With `tui.mouse: off` the terminal keeps the mouse for its own selection and scrollback, and the TUI SHALL handle neither the wheel nor clicks
- **inline**: append-only scrollback with a repaintable footer, like Codex and OpenCode mini

The system SHALL copy selections with OSC 52 when the native clipboard is unavailable. It SHALL set the terminal title to `cyber · <session name> · <status>` (configurable by `tui.title.fields`). On exit it SHALL print a summary with tokens, cost and the resume command `cyber -r <name|id>`.

#### Scenario: Exit summary
- **WHEN** the user quits a Session named `auth-refactor`
- **THEN** the terminal shows the total cost and tokens, and `Resume with: cyber -r auth-refactor`

#### Scenario: Clipboard over SSH
- **WHEN** the user copies a code block over SSH with no native clipboard available
- **THEN** the TUI emits an OSC 52 sequence so the local terminal receives the text

#### Scenario: Wheel scrolls the transcript
- **WHEN** the user turns the mouse wheel up over the fullscreen TUI
- **THEN** the transcript scrolls back 3 lines per notch and the terminal's own scrollback does not move

#### Scenario: Mouse left to the terminal
- **WHEN** `tui.mouse` is `off`
- **THEN** the TUI does not enable mouse reporting and the terminal handles the wheel and selection itself

#### Scenario: Wheel over the sidebar
- **WHEN** the sidebar is docked and the user turns the wheel over it
- **THEN** neither the transcript nor the sidebar moves

### Requirement: Message and part rendering
(P0) The system SHALL render:
- assistant text as Markdown with syntax-highlighted code blocks (P1: as defined in Markdown rendering and Syntax-highlighted code blocks)
- reasoning collapsed by default; Ctrl+T toggles it per Session
- each tool call as a collapsible card with title, status (`pending`, `running`, `completed`, `failed`, `interrupted`), duration and summary
- file edits as diffs, in `unified` or `split` style (`tui.diff_style`, default `auto`: split when the terminal is at least 160 columns wide)

`/timestamps` SHALL toggle per-message timestamps. Tool output over 30 lines SHALL be collapsed to its last 10 lines until expanded. (P1) Diff lines SHALL use the `diff_*` theme tokens with `diff_added_bg` and `diff_removed_bg` backgrounds, and each finished run SHALL end with the Turn footer marker.

(P1) With the mouse captured, a click on a collapsed tool output's hidden-lines row SHALL act as the `tool_details` key (Ctrl+O), and a click on a collapsed `thinking` row SHALL act as the `reasoning_toggle` key (Ctrl+T); neither row is a target while its key is unbound. Links SHALL stay the terminal's to open through OSC 8.

#### Scenario: Split diff on wide terminal
- **WHEN** an edit completes and the terminal is 200 columns wide with `diff_style` set to `auto`
- **THEN** the edit card renders a side-by-side diff

#### Scenario: Long bash output collapsed
- **WHEN** a bash tool returns 400 lines
- **THEN** the card shows the last 10 lines and `+390 lines (press o to expand)`

#### Scenario: Click a collapsed tool output
- **WHEN** a bash card shows its last 10 lines under a hidden-lines row and the user clicks that row
- **THEN** tool output expands exactly as after Ctrl+O

### Requirement: Permission prompt
(P0) The system SHALL show a blocking prompt for each pending permission request of the active Session tree. The prompt SHALL show the tool title, the resources, and a diff preview for edits. It SHALL offer:
- **Allow once**
- **Allow always**, followed by a confirmation step listing the exact save patterns and their scope (project or user)
- **Reject**, with an optional feedback message sent back to the model

Requests SHALL be shown one at a time in arrival order.

(P1) With the mouse captured, a click on **Allow once**, **Allow always…** or **Reject** SHALL only select it. A second click on the same option, with no key pressed between and while the same request is pending, SHALL act as Enter on it. A single click SHALL never reply to a request, and a press that comes within 500 ms of a press made before the request was shown SHALL be absorbed as the rest of that multi-click: it SHALL neither select nor arm an option. The Allow always confirmation step, the rejection feedback and the blocked-call approval SHALL take no clicks.

#### Scenario: Allow always confirmation
- **WHEN** bash requests `npm run test -- --watch=false` and the user picks Allow always
- **THEN** the TUI shows `Always allow: bash "npm run test *" (project)` and saves the approval only after the user confirms

#### Scenario: Reject with feedback
- **WHEN** the user rejects an edit and types `use the existing helper instead`
- **THEN** the reply is `reject` with that message, and the model receives it as a tool error instead of the Drain halting

#### Scenario: A single click cannot grant
- **WHEN** bash asks to run `rm -rf build` and the user clicks `Allow once` once
- **THEN** `Allow once` is selected and no reply is sent; a second click on it replies `once`

#### Scenario: A triple click does not answer the prompt it raised
- **WHEN** the user triple-clicks an explorer file behind an `ask` rule, and the permission request is shown under the pointer after the first press
- **THEN** the other two presses select nothing and no reply is sent; a later double click on `Allow once` replies `once`

#### Scenario: Armed click does not carry over
- **WHEN** the user clicks `Allow once` on one request, that request is answered from another client, and the next request appears where the click lands again
- **THEN** the click only selects `Allow once` on the new request and sends no reply

### Requirement: Question prompt
(P0) The system SHALL render `question` tool requests as an inline form:
- a single-select question submits on selection
- multiple questions use tabs with a final review step
- questions that allow custom answers offer `Type your own answer`

Esc SHALL dismiss the request, which halts the Drain as defined in `session-runtime`.

(P1) With the mouse captured, a click on an option or on `Type your own answer` SHALL only move the cursor there, and a second click on the same row SHALL act as Enter on it. A single click SHALL never submit or dismiss the request.

#### Scenario: Multi-question tabs
- **WHEN** the agent asks two questions, `Database?` and `Auth method?`
- **THEN** the TUI shows two tabs plus a Review tab and sends both answers together on confirm

#### Scenario: Answer by clicking twice
- **WHEN** the agent asks `Database?` with `Postgres` and `SQLite` and the user clicks `SQLite` once
- **THEN** the cursor is on `SQLite` and nothing is sent; a second click submits `SQLite` as Enter would

### Requirement: Background tasks view
(P1) The system SHALL provide `/tasks` (Codex alias `/ps`), listing every background item of the current Session with status, elapsed time and last output line:
- background shell commands (`job_`)
- background subagents (`agt_`)
- monitors
- Workflow Runs (`run_`)
- Loops (`lop_`)

From the list the user SHALL be able to view full output, attach to a subagent thread, or stop one item. `/stop` SHALL stop all background shell commands after confirmation.

(P1) With the mouse captured, a click SHALL select a row and a second click on it SHALL act as Enter. Stopping stays on its keys.

#### Scenario: Stop one background command
- **WHEN** the user selects a running `npm run dev` job and presses `x`
- **THEN** the job is terminated, its status becomes `cancelled`, and the model is notified at the next Safe Boundary

#### Scenario: Open a task's output by clicking
- **WHEN** `/tasks` lists `npm run dev` and the user clicks its row twice
- **THEN** the task's output opens as with Enter, and the job keeps running

#### Scenario: Keys of the tasks list
- **WHEN** the tasks list is open
- **THEN** `x` SHALL stop the selected item while no filter text is typed (Ctrl+S stops it at any time), and typing otherwise filters the list
- **AND** Enter SHALL open a subagent's thread, a Workflow Run in the Workflow monitor, or the end of a command's or monitor's log in a scrollable view
- **AND** Loops SHALL be listed once the P2 `loops-scheduling` capability exists; until then there are none to list

#### Scenario: Background input and Ctrl+B
- **WHEN** the composer input starts with `&`
- **THEN** the input mode SHALL read `background shell`, and submitting it SHALL start the rest as a background Job of the Session
- **WHEN** the user presses Ctrl+B (the rebindable `background_command` binding)
- **THEN** a running Session's foreground `bash` call SHALL move to the background, and with no Turn running the TUI SHALL say `No command is running` and send nothing

### Requirement: Subagent threads and side chat
(P1) The system SHALL provide `/agent` (alias `/subagents`) to switch the view to any running or finished subagent thread of the Session, show its transcript, and send it messages when it is resumable. (P1) `/btw <question>` (alias `/side`) SHALL open an ephemeral overlay chat. The side chat SHALL see the current Session context read-only, SHALL NOT add messages to the main Session, and SHALL be discarded on close unless the user chooses `Insert answer`.

(P1) With the mouse captured, a click in the `/agent` list SHALL select a thread and a second click on it SHALL switch to it as Enter does.

#### Scenario: Side question does not pollute history
- **WHEN** the user asks `/btw what does the --frozen flag do?` while the main agent works
- **THEN** the answer appears in an overlay and the main Session history gains no message

#### Scenario: Switch thread by clicking
- **WHEN** `/agent` lists `review` and `explore` and the user clicks `explore` twice
- **THEN** the view switches to the `explore` thread

#### Scenario: Overlay keys
- **WHEN** the side chat overlay shows an answer
- **THEN** Esc SHALL discard it, and `i` or Enter (`Insert answer`) SHALL close the overlay and insert the answer into the composer for the user to edit and send
- **AND** a bare `/btw` SHALL only show its usage, and `/btw` SHALL be listed in the palette, the help and `GET /api/v1/commands`

### Requirement: Workflow monitor
(P2) The system SHALL provide `/workflows` (also from the command palette and from the sidebar Workflows section). It SHALL list every Workflow Run of the attached Location, active runs first and then the newest. `a` SHALL toggle between this Location and all Locations.

Each row SHALL show the name, status (`paused (<n> settling)` while a paused run's agents finish, `#<n> in queue` while queued), current phase, a known-progress nyan bar of completed over total agents, agents `done/running/queued/failed`, tokens, cost against budget, and active elapsed time. The bar SHALL animate only on the selected running row. Rows SHALL update live from `workflow.run.progress` and the `workflow.*` events, and the list SHALL be re-read when the event stream reconnects.

Enter on a run SHALL open its run view: the agent tree grouped by phase (each phase with `done/total`, plus its running and queued counts), with one row per agent showing status, label or ID, tier or model, tokens, cost, duration and branch. Enter or `t` on an agent SHALL open its child Session transcript with live updates, and Esc SHALL return to the run view and then to the list.

Keys:
- `p` SHALL pause a running or queued run, or resume a paused one.
- `r` SHALL resume (relaunch) a paused, failed, stopped or budget-exceeded run, offering a new revision when the script changed.
- `x` SHALL stop a run after confirmation.
- `R` SHALL resume every run interrupted by a restart.
- `s` SHALL open the run's script.
- `l` SHALL show its log.

Every action SHALL use the public `workflow` routes, act on the selected run only, and report a refusal (for example `Run has agents still exiting; retry shortly`) on the row.

A running Workflow Run SHALL show a compact progress line above the composer, as "Workflow run indicators" defines.

At 80 columns, phase and elapsed time SHALL be dropped. Below 60 columns, each run SHALL take two rows. Screen readers SHALL receive each row as one sentence with every value in words.

(P2) With the mouse captured, a click on a run or an agent row SHALL select it and a second click on it SHALL act as Enter (open the run view, or the agent's transcript). Pause, resume and stop stay on their keys.

#### Scenario: Pause a run
- **WHEN** the user presses `p` on a running Workflow Run with 6 agents in flight
- **THEN** no new agents start, in-flight agents finish, and the run shows `paused (6 settling)` and then `paused`

#### Scenario: Budget display
- **WHEN** a run has spent $1.80 of a $5.00 budget
- **THEN** the monitor shows `$1.80 / $5.00` and colors it warning once spend exceeds 80%

#### Scenario: Several runs at once
- **WHEN** three runs are running and one is queued in the Location
- **THEN** `/workflows` lists all four with their live counts, and pausing one leaves the other rows unchanged

#### Scenario: Drill into an agent
- **WHEN** the user presses Enter on `audit` and then `t` on a running verify agent
- **THEN** the agent's child Session transcript opens and streams, and Esc returns to the `audit` agent tree with the agent still selected

#### Scenario: Open a run by clicking
- **WHEN** `/workflows` lists `audit` and `fanout` and the user clicks `fanout` twice
- **THEN** the first click selects `fanout` and the second opens its run view

### Requirement: Command palette
(P1) The system SHALL open a command palette with the `command_palette` keybinding (default Ctrl+P), and with `/` followed by Tab in an empty composer. It SHALL list every built-in TUI command, custom command, skill and MCP prompt, plus UI actions (toggle sidebar, open settings tabs, switch theme, copy last code block, toggle reasoning). Each row SHALL show the name, a one-line description and the current key binding. A command that declares an argument hint SHALL be typed into the composer as `/name ` instead of run. Names and descriptions from commands, skills and MCP prompts SHALL be shown on one line without control characters. Typing SHALL filter by fuzzy match, Enter SHALL run the selected entry, and Esc SHALL close the palette without side effects.

With the mouse captured, a click SHALL select an entry and a second click on it SHALL run it as Enter does. A click outside the palette SHALL leave it open.

#### Scenario: Open settings from the palette
- **WHEN** the user presses Ctrl+P, types `usage` and presses Enter
- **THEN** the Settings panel opens on the Usage tab

#### Scenario: Run an entry by clicking
- **WHEN** the palette filters to `Settings: Usage` and the user clicks that entry twice
- **THEN** the Settings panel opens on the Usage tab

### Requirement: Session sidebar
(P1) The system SHALL provide a right sidebar, toggled by `<leader>b` and `/sidebar`, with `tui.sidebar` set to `auto` (default), `show` or `hide`. In `auto` and `show` it SHALL be docked when the terminal is at least 120 columns wide; `hide` SHALL start hidden, and the toggle SHALL flip the docked state for the rest of the run. Its width SHALL be 30% of the terminal, clamped to 32..48 columns. From top to bottom it SHALL show:
- the Session title, and the parent title for child Sessions
- **Context**: a `Context` row with a one-row eighth-block bar of the context window used (the same glyphs and `border` track as the Machine bars; `success` below 70%, `warning` from 70%, `error` from the auto-compaction point, or from 90% when auto-compaction is off) and the percent, with a thin `│` marker cell at the auto-compaction point; under it `used / window tokens`, then `│ auto-compact at N%` (`auto-compact pending` in `error` at or above the point, `compacting…` in `warning` while compaction runs, `auto-compact off` without a point, and no marker then), then the cost spent (`—` when unpriced, `≥ $x` when partly unpriced). The numbers SHALL match the footer's `29.5K (22%)`. Before the first reply it SHALL show `0 / <window> tokens · <model>`; after a compaction, `compacted · measured after the next reply`; with an unknown window, `N tokens · window unknown` without a bar or percent. Under `NO_COLOR` the bar SHALL be `[####......]` text with `|` at the marker. Screen readers get the sentence `Context 22 percent used, 29.5 thousand of 128 thousand tokens, auto-compact at 85 percent` in the Status tab
- **MCP**: shown whenever `GET /api/v1/mcp` answers, as a collapsible list (`▼`/`▶`) with one row per configured server: a status dot, the name, the transport (`remote`/`stdio`) in `text_subtle` and the status word (`Connected · N tools`, `Connecting`, `Idle`, `Failed`, `Needs auth`, `Disabled`, `Blocked` for a server blocked by organization policy); a failed server shows its sanitized error and a server needing auth shows `cyber mcp auth <name>`, each truncated to one row; at most six servers are listed, then `+N more · /status`; with no configured server it shows `No MCP servers` and `add one: cyber mcp add`
- **LSP**: one row per language server with its status (`starting`, `connected`, `broken`), or `LSPs will activate as files are read` when none has started
- **Modified files**: files changed in this Session with `+adds -dels` (from `GET .../diff` without `message_id`, the diff from the Session's first pre-Turn snapshot to its latest post-Turn snapshot), at most 10 rows and then `+N more`; omitted when nothing changed
- at the bottom: `<cwd>:<branch>` (home shown as `~`, worktree name when in a managed worktree, branch omitted outside git), then a connection dot (green connected, yellow reconnecting, red disconnected) with `Cyber Code <version>`

MCP rows SHALL update from `mcp.status.changed.1` and `mcp.tools.changed.1`, Modified files from `session.diff.1`, and LSP and the branch SHALL be re-read when the Session goes idle (language servers start as files are read), all without polling; every section SHALL be re-read when the event stream reconnects. A dropped event stream SHALL be reopened every 2 seconds, with the connection dot yellow while it retries and red after a failed attempt. `<leader>e` SHALL move focus into the sidebar, opening the drawer below 120 columns: Up/Down (and `j`/`k`) move between rows, Enter or Space collapses or expands MCP or Modified files, Enter on a modified file opens its diff, `r` on a failed or `needs_auth` MCP server reconnects it through `POST /api/v1/mcp/{name}/reconnect`, and Esc returns focus to the composer (closing the drawer). These keys SHALL be the `sidebar` context's bindings `sidebar_next`, `sidebar_previous`, `sidebar_select`, `sidebar_reconnect` and `sidebar_close`. The reconnect route SHALL accept a server that settled `failed` only when it retained no native owner, so a failure with unverified ownership stays refused with `409`. All server-provided names, errors, paths and patches SHALL be shown on one line (patches per line) without control characters. Every value SHALL come from public API routes. A section whose source route is unavailable SHALL be omitted, never shown with placeholder values.

(P1) With the mouse captured, a click on a section header (MCP, Modified files, Machine) SHALL focus the sidebar on it and fold or unfold the section as Enter does. A click on another focusable row SHALL focus the sidebar on that row, and a second click on it SHALL act as Enter. A click on the transcript or the composer SHALL return focus to the composer as Esc does, closing the drawer. The wheel SHALL do nothing over the sidebar.

#### Scenario: Wide terminal shows the sidebar
- **WHEN** the terminal is 160 columns wide and `tui.sidebar` is `auto`
- **THEN** the sidebar is drawn at 48 columns with Context, MCP, LSP and the `cwd:branch` footer

#### Scenario: Context early in a Session
- **WHEN** the last reply's prompt was 28.2K tokens of a 128K window that auto-compacts at 85%
- **THEN** the Context bar is green at 22% with the marker at 85%, followed by `28.2K / 128K tokens` and `│ auto-compact at 85%`, and the footer reads `28.2K (22%)`

#### Scenario: Context getting full
- **WHEN** the prompt reaches 78% of the window
- **THEN** the bar is drawn in `warning`

#### Scenario: Auto-compaction pending
- **WHEN** the prompt reaches 91% of a window that auto-compacts at 85%
- **THEN** the bar is drawn in `error` and the line reads `auto-compact pending`

#### Scenario: Context before the first reply
- **WHEN** a new Session on a 400K-window model has not replied yet
- **THEN** the Context section shows an empty bar at 0% and `0 / 400K tokens · <model name>`, and the footer shows no context field

#### Scenario: Unknown context window
- **WHEN** the catalog does not know the model's window and the last reply used 29,470 prompt tokens
- **THEN** the Context section reads `29,470 tokens · window unknown` with no bar and no percent

#### Scenario: Failed MCP server
- **WHEN** MCP server `context7` fails to start with `spawn ENOENT`
- **THEN** its row shows a red dot, `Failed` and `spawn ENOENT`, and `r` on that row reconnects it

#### Scenario: No language servers yet
- **WHEN** no language server has started in the Location
- **THEN** the LSP section reads `LSPs will activate as files are read`

#### Scenario: Fold a section by clicking
- **WHEN** the MCP section is expanded and the user clicks its `▼ MCP` header
- **THEN** the section folds to `▶ MCP` and the sidebar has focus on that header

#### Scenario: Click back into the composer
- **WHEN** the sidebar has focus and the user clicks the composer
- **THEN** focus returns to the composer and typed keys go to the prompt

#### Scenario: Click a collapsed row while the sidebar has focus
- **WHEN** the sidebar has focus and the user clicks a tool output's `lines hidden` row
- **THEN** focus returns to the composer and the tool output expands as Ctrl+O does

### Requirement: Settings panel
(P1) The system SHALL provide a full-screen Settings panel with the tabs Status, Config, Usage and Stats. `/settings` SHALL open the last used tab, and `/status`, `/config`, `/usage` and `/stats` SHALL open their own tab. `/cost` SHALL open the Usage tab scrolled to the Session section. The panel SHALL replace the transcript and composer while open, keep the recap block when one is shown, and draw the tab bar as `Settings  Status  Config  Usage  Stats`, with the active tab in `selection` (reverse video under `NO_COLOR`). Keys: Tab and Shift+Tab or Left and Right switch tabs, `1`–`4` jump to a tab, Up/Down, PageUp/PageDown, Home and End scroll, `r` refreshes, and Esc closes. The bottom row SHALL list the keys of the current tab. Each tab SHALL load its data when it is shown, show `Loading…` until the data arrives, and show a failed request as `Could not load <section>: <error> · r to retry` without closing the panel. A Drain keeps running while the panel is open, and a permission or question request SHALL close the panel and show its prompt.

With the mouse captured, a click on a tab in the tab bar SHALL switch to it as its number key does, and the wheel SHALL scroll as Up/Down do.

#### Scenario: Direct tab command
- **WHEN** the user types `/stats`
- **THEN** the Settings panel opens with the Stats tab active

#### Scenario: Approval interrupts the panel
- **WHEN** a permission request arrives while the Usage tab is open
- **THEN** the panel closes and the permission prompt is shown

#### Scenario: Click a tab
- **WHEN** the Settings panel shows the Status tab and the user clicks `Usage` in the tab bar
- **THEN** the Usage tab is active and loads its data

### Requirement: Settings Config tab
(P1) The Config tab SHALL list TUI preferences that can be edited in place: `theme`, `theme_mode`, `sidebar`, `recap`, `spinner`, `reasoning`, `diff_style`, `timestamps`, `color` and `accessibility.reduced_motion`. Each row SHALL show the current value and its source (`default`, `global`, `project`, `saved` for `kv.json`, or `session` for a toggle made in this run only, such as Ctrl+T or `/timestamps`). Enter or Space SHALL cycle a value or open its picker. A change SHALL apply immediately and be saved to `~/.local/state/cyber/kv.json` with the time it was saved, and a `tui.jsonc` written after that time SHALL win over it. The tab SHALL also list an `output style` row. It SHALL show the current Session's effective output style and its source (`session`, `config` or `default`), and Enter SHALL open the output style picker. Below the preferences, the tab SHALL show effective core settings (`model`, `small_model`, `mode`, `default_agent`, `output_style`, `budgets`) read-only with their source file, when the server's `config` route is available. Otherwise it SHALL show `Core settings: use cyber config or edit cyber.jsonc`.

With the mouse captured, a click on a preference row SHALL select it, and a second click on it SHALL act as Enter (cycle the value or open its picker).

#### Scenario: Toggle the sidebar from Config
- **WHEN** the user selects `sidebar` and presses Enter until it reads `hide`
- **THEN** the sidebar disappears, and the value persists after a restart

#### Scenario: Output style from Config
- **WHEN** the user selects the `output style` row and presses Enter
- **THEN** the output style picker opens, and choosing `concise` updates the row to `concise (session)`

#### Scenario: Cycle a value by clicking
- **WHEN** the user clicks the `timestamps` row once and then again
- **THEN** the first click selects it and the second cycles it to its next value, which is saved

### Requirement: File explorer
(P1) The TUI SHALL provide a file explorer, opened with `/files` or the `file_explorer` binding (default `<leader>o`, that is `ctrl+x o`, because `<leader>e` is `sidebar_focus`) and closed with Esc. It SHALL show the Location's project as a tree in a pane on `surface_alt`: a header bar (`surface`) with `EXPLORER`, the project name and `:branch`, and at the right `✓ clean` in `success` or `N changed` in `warning`; a filter line with a `⌕` placeholder; the tree; and a summary line (`N shown · M hidden · .gitignore`). Root entries SHALL have no guide; each deeper level SHALL add a `│ ` indent guide in `text_subtle`, followed by the fold marker (`▸` collapsed, `▾` expanded). Directory names SHALL be bold in `file_dir` with a trailing `/`, an expanded directory SHALL show its loaded child count, and file names SHALL be colored by kind from their name: `file_doc` (Markdown and text), `file_config` (TOML, JSON, YAML, `justfile` and similar), `file_code` (source files) and `file_dim` (hidden, ignored, lock and generated files). A right-aligned column SHALL show the git status letter and the compact size (`1.2K`) in `text_muted`. The selected row SHALL have an `accent` bar and the `selection` background (reverse video without color). `tui.explorer.icons` (`none` default, `nerd`) SHALL add Nerd Font glyphs before names. The footer SHALL show the explorer keys as chips (the key on a `text_muted` background, then its label), generated from the binding table. Directories SHALL sort before files and SHALL be loaded lazily, one level per expansion, through `GET /api/v1/files`. Each row SHALL show the git status letter (`M` in `warning`, `A` and `?` in `success`, `D` in `error`) from the same VCS data as the sidebar, and a directory SHALL show `M` when any descendant changed. `.gitignore` and `.cyberignore` SHALL apply by default; `.` SHALL toggle hidden and ignored entries, and the footer SHALL count what is hidden. `/` SHALL filter by fuzzy path match across already-loaded and server-searched entries, and `g` SHALL show only changed files.

Enter on a file SHALL open a preview pane to the right, or replace the tree when the terminal is narrower than 100 columns. (P1) The `explorer_focus` binding (default Tab) SHALL move focus between the tree and an open preview, Esc SHALL return focus from the preview to the tree, and the focused pane's header title SHALL be drawn in `accent`. While the preview has focus, Up/Down, PageUp/PageDown and Home SHALL scroll it, and below 100 columns it SHALL replace the tree. The preview SHALL have its own header bar with a breadcrumb (`docs › README.md`), the language, line count and size, and the `+N −M` diff summary. A vertical divider SHALL separate it from the tree. Markdown files SHALL be rendered through the transcript's Markdown renderer by default (no OSC 8 links are written from the preview), and `m` SHALL toggle the raw source, shown by a `rendered`/`raw` chip in the header. Source (code, or raw Markdown with markup scopes colored, never one color) SHALL have line numbers, a gutter mark on changed lines and a `│` separator, with syntax highlighting through the same highlighter and theme tokens as code blocks; long lines SHALL soft-wrap with a hanging indent under the gutter. A scrollbar thumb SHALL show the position when the preview is longer than the pane. `←` SHALL fold the selected directory or move to its parent, and `→` SHALL unfold it. `@` SHALL insert the selected path as a file mention in the composer, relative to the Session directory (absolute when the file lies outside it, as when the Location is below the project root), `y` SHALL copy the project-relative path, and `r` SHALL refresh. The explorer SHALL refresh changed rows on file-change and VCS events without collapsing the user's expansion state.

The explorer SHALL never bypass permissions. Listing SHALL be evaluated as `list` and preview as `read` against the Session's rules for that path. A file whose `read` decision is `ask` SHALL appear dimmed with `⊘ ask to read`, and opening its preview SHALL raise the normal permission request. A file whose decision is `deny` SHALL appear dimmed with `⊘ denied`, with no content. Preview SHALL be capped at 1 MiB and 5,000 lines, with a truncation notice. Binary files SHALL show `binary · <size>`, and symlinks SHALL show their target, never followed outside the project. Displayed file names and content SHALL be sanitized like other untrusted output, so no terminal escape sequence reaches the terminal. Screen readers SHALL get each row as `<name>, directory|file, <status>, level <n>`.

(P1) With the mouse captured, a click on a tree row SHALL select it and act as Enter: a directory row, or its fold marker, folds or unfolds it, and a file row opens its preview (an `ask` file raises the permission request and a `deny` file only shows its notice, as with Enter). A click in the preview SHALL focus it and a click in the tree pane SHALL focus the tree, as `explorer_focus` does. A click on the header's `rendered`/`raw` chip SHALL act as `m`, and a click on a key chip SHALL act as its key. The wheel SHALL act on the pane under the pointer, whichever has focus: over the tree it SHALL move the selection, and over the preview it SHALL scroll the preview by 3 lines per notch.

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

#### Scenario: Browse by clicking
- **WHEN** the user clicks the folded `crates/` row and then the `README.md` row
- **THEN** `crates/` unfolds, and `README.md` is selected and its preview opens

#### Scenario: Wheel over the preview
- **WHEN** a long file's preview is open beside the tree and the user turns the wheel down over the preview
- **THEN** the preview scrolls 3 lines and the tree selection does not move

#### Scenario: Focus the preview
- **WHEN** a preview is open beside the tree and the user presses Tab, or clicks the preview
- **THEN** the preview's header title is drawn in `accent` and Down scrolls the preview; Esc returns focus to the tree

#### Scenario: Toggle raw by clicking the chip
- **WHEN** `docs/README.md` is previewed under a `rendered` chip and the user clicks the chip
- **THEN** the preview shows the Markdown source and the chip reads `raw`

### Requirement: Theme switcher
(P1) `/theme`, the palette action `Theme…` and the Config tab's theme row SHALL open the theme switcher, a two-pane overlay. The left pane SHALL list built-in and custom themes grouped under `DARK` and `LIGHT` by the theme's resolved background luminance. Each row SHALL show the theme name, `current` on the active theme, `custom` on themes loaded from `themes/*.json`, and a swatch of its `accent`, `secondary`, `success`, `warning` and `error` tokens drawn on the theme's own background. Tab SHALL cycle the filter `All · Dark · Light`, and typing SHALL fuzzy-search names. The right pane SHALL show a live preview rendered with the highlighted theme's tokens: the welcome logo gradient, a Markdown heading, a list and a link, a highlighted code block, an added and a removed diff line, success, warning and error status glyphs, a Turn footer marker, and the measured contrast of `text` and `text_muted` against `background` with the WCAG level. Moving the selection SHALL apply the theme to the whole TUI immediately. Enter SHALL keep it and persist it to `kv.json`, `p` (with an empty search, since typing searches) SHALL write `theme` to the `.cyber/tui.jsonc` of the project root, refusing a file with comments rather than rewriting it, and Esc SHALL restore the theme that was active when the switcher opened. The `theme_cycle` binding (default `ctrl+x t`) SHALL switch to the next theme in the current filter without opening the switcher, keep it in `kv.json`, and SHALL announce `Theme: <name>` in the status footer for 2 seconds; `theme_list`, which opens the switcher, moves to `<leader>s`. A permission or question request that replaces the switcher SHALL restore the theme it opened with. Under `NO_COLOR` the switcher SHALL list themes and explain that colors are disabled, with no swatches. A theme whose `text` contrast is below 4.5:1 SHALL show `low contrast` in `warning` in the preview.

The built-in themes SHALL include the existing set plus `cyber-light`, `synthwave-84`, `rose-pine`, `kanagawa`, `ayu-mirage` and `matrix`, each defining every token or deriving it by the documented derivation rules, and each passing the contrast test.

With the mouse captured, a click on a theme row SHALL select it, which previews it as moving the selection does, and a second click on it SHALL keep it as Enter does. A click outside the switcher SHALL neither close it nor restore the previous theme.

#### Scenario: Live preview and revert
- **WHEN** the user opens `/theme`, moves to `synthwave-84` and presses Esc
- **THEN** the TUI showed `synthwave-84` while it was selected and returns to the previous theme, and nothing is persisted

#### Scenario: Apply and persist
- **WHEN** the user selects `rose-pine`, presses Enter and restarts the TUI
- **THEN** the TUI starts with `rose-pine`

#### Scenario: Cycle without the switcher
- **WHEN** the user presses `ctrl+x t` twice with the filter `Dark`
- **THEN** the theme moves two dark themes forward and the footer shows `Theme: <name>`

#### Scenario: Preview and keep by clicking
- **WHEN** the user clicks `rose-pine` once and then again
- **THEN** the first click previews `rose-pine` and the second keeps it and persists it to `kv.json`

### Requirement: Branch tree view
(P1) The system SHALL provide `/tree`, listing the current Session's user messages as a collapsible tree. The tree SHALL highlight the active path, show the number of branches at each branch point, and show labels as tags. Keys in the tree:

- **Enter** SHALL check out the selected branch.
- **`c`** SHALL toggle restoring code with the checkout, with the same diff preview as `/rewind`.
- **`l`** SHALL label the selected message.

`/label <name>` SHALL label the active tip, and `/label --clear <name>` SHALL remove a label. When the Session has more than one tip, the header SHALL show `branch <label or short id> (<n> of <total>)`. `/tree` and `/label` SHALL appear in `/help` and in the command palette (**Session tree**, **Label branch**).

With the mouse captured, a click SHALL select a message and a second click on it SHALL act as Enter, with the same idle check.

#### Scenario: Switch branches from the tree
- **WHEN** the Session has two branches after a rewind and the user selects the inactive one in `/tree` and presses Enter
- **THEN** the transcript shows that branch's messages and the header shows `branch <id> (1 of 2)`

#### Scenario: Busy Session
- **WHEN** the user presses Enter in `/tree` while a Turn is running
- **THEN** the TUI reports that the Session must be idle and the active branch does not change

#### Scenario: Check out by clicking
- **WHEN** the Session is idle and the user clicks the inactive branch's message twice in `/tree`
- **THEN** that branch is checked out as with Enter

## ADDED Requirements

### Requirement: Mouse clicks
(P1) In fullscreen mode with the mouse captured, the TUI SHALL act on left button presses. Releases, drags, the right and middle buttons, and presses with a modifier SHALL be ignored, so a modifier drag stays the terminal's own selection. Click targets SHALL come from the last drawn frame, and a target whose row no longer exists SHALL do nothing. Each target SHALL be one of:
- a **row** of a list: the pickers (Session, model, mode, `/agent`, `/tasks`, skills, plugins, loops, goals and the others built on the generic picker), the command palette, the output style picker, the theme switcher, the Workflow monitor, `/messages`, `/team`, `/tree`, the Settings Config rows and the options of a permission or question prompt. The first click SHALL select the row and arm it; a click on the armed row SHALL activate it as Enter does, and only when it still shows the item the first click selected: when the list reloads or reorders in between, or the picker is replaced by another kind, the click SHALL only select and arm the row now there. A list scrolled past its height SHALL keep its window until the selection leaves it, so a clicked row stays under the pointer. A double click (a second press on the same target within 500 ms) SHALL therefore select and then activate. The hook views draw no rows to pick (`/hooks` shows one definition at a time, and Enter closes `/hooks history`), so they SHALL take no clicks; the wheel still moves them.
- a **control**: a Settings tab, a key chip, the explorer's `rendered`/`raw` chip, an explorer tree row, a sidebar section header, or a collapsed transcript row. One click SHALL act as its key does. The second press of a double click on a control (a press on the same control, or at the same cell, within 500 ms) SHALL be ignored, so a double click toggles once.
- a **pane**: the transcript, the composer, and the explorer's tree and preview. A click SHALL move focus to it as its key does.

Any key press, a change of the open panel, and a new pending request SHALL disarm the armed row. Every click SHALL do only what a key already does in that place, through the same handler, and nothing reachable by the mouse SHALL be unreachable by the keyboard. While a modal panel is open (a picker, the palette, the theme switcher, a prompt, a confirmation, help or a full-screen panel), a click outside it SHALL do nothing and SHALL never close it, and a click inside it on no target SHALL do nothing. Confirmation dialogs (stop background tasks, bypass mode, overwrite an export), the Allow always confirmation step, the rejection feedback and the blocked-call approval SHALL take no clicks. `tui.mouse: off`, inline rendering and screen-reader mode SHALL handle no clicks.

#### Scenario: Second click activates a row
- **WHEN** the model picker is open and the user clicks `gpt-5` once, then clicks it again
- **THEN** the first click selects `gpt-5` and the second switches the Session's model to it, as Enter would

#### Scenario: A key between clicks disarms
- **WHEN** the user clicks a Session row in `/resume`, presses Down and Up, and clicks the same row again
- **THEN** the second click only selects the row again and nothing is resumed

#### Scenario: Double click on a control toggles once
- **WHEN** the user double clicks the folded `src/` row in the file explorer
- **THEN** `src/` unfolds once and stays unfolded

#### Scenario: Click outside a modal panel
- **WHEN** the command palette is open and the user clicks the transcript beside it
- **THEN** the palette stays open with the same selection and nothing else changes

#### Scenario: Confirmations take no clicks
- **WHEN** `Stop all background commands?` is shown and the user clicks on it twice
- **THEN** nothing is stopped and the confirmation stays until a key answers it

#### Scenario: Mouse off
- **WHEN** `tui.mouse` is `off` and the terminal still delivers a click on a palette row
- **THEN** the TUI ignores it
