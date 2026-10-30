# tui Specification

## Purpose
The terminal UI is the primary interactive client of Cyber Code: a Rust (ratatui) program that talks to the local `cyber` server through the same public API as every other client. It combines OpenCode's server-backed TUI (session picker, themes, keybinds, permission prompts), Codex's fast ergonomic composer (`/side`, status line, `/ps`), and Claude Code's orchestration views (agent view, workflow monitor, goal status, rewind menu, steer/queue while running). Because it uses only the public API, anything the TUI can do can also be scripted or done from a remote Device.

## Requirements

### Requirement: Launch and server transport
(P0) The system SHALL start the TUI with `cyber [project]`, resolving `project` relative to the current directory (default: the current directory). By default it SHALL connect to the registered background server, starting it as `cyber service start` does when none is registered. `--embedded` SHALL run a private in-process server with no listener on a private database, for tests and single-process offline use, and SHALL never open the shared database for writing. `cyber attach <url>` SHALL connect to a remote server. Every transport SHALL use only the public `/api/v1` routes and SSE/WebSocket event streams.

#### Scenario: Default in-process launch
- **WHEN** a user runs `cyber --embedded --ephemeral` in `/repo`
- **THEN** the TUI starts an in-process server for Location `/repo` without opening a TCP listener and writes nothing to the shared database

#### Scenario: First launch starts the service
- **WHEN** a user runs `cyber` in `/repo` with no background service registered
- **THEN** the TUI starts the background server, connects to it for Location `/repo`, and renders the home screen within 500 ms of the server reporting ready

#### Scenario: Attach to the background service
- **WHEN** `cyber service status` reports `running http://127.0.0.1:4747` and the user runs `cyber`
- **THEN** the TUI connects to that server with the stored service credentials instead of starting a new one

#### Scenario: Attach to a remote server
- **WHEN** a user runs `cyber attach https://box.example:4747 --cwd /srv/repo`
- **THEN** the TUI authenticates with the configured credentials and opens Location `/srv/repo` on the remote server

### Requirement: Launch flags
(P0) The system SHALL accept these launch flags:
- `--continue`/`-c`: resume the most recent root Session of the Location.
- `--resume [id|name]`/`-r`: resume a Session by ID or name, or open the picker when no value is given.
- `--fork`: copy the selected Session into a new one.
- `--model provider/model[#variant]`, `--agent <name>`, `--mode <mode>`.
- `--worktree [name]`: start in a managed worktree.
- `--prompt <text>`: submit an initial prompt.
- `--dangerously-skip-permissions`: with `--embedded`, start in `bypass` with the override of `--dangerously-bypass-permissions`.

Piped stdin SHALL be prepended to `--prompt`. Invalid combinations SHALL fail before the UI renders, with exit code 2, including `--dangerously-skip-permissions` without `--embedded` or with a `--mode` other than `bypass`.

#### Scenario: Fork requires a source session
- **WHEN** a user runs `cyber --fork` without `--continue` or `--resume`
- **THEN** the CLI prints `--fork requires --continue or --resume` and exits with code 2 without rendering the UI

#### Scenario: Piped stdin becomes the prompt
- **WHEN** a user runs `git diff | cyber --prompt "review this"`
- **THEN** the first submitted message contains the diff text followed by `review this`

#### Scenario: Resume by name
- **WHEN** a user runs `cyber -r auth-refactor` and exactly one Session in the Location is named `auth-refactor`
- **THEN** the TUI opens that Session with its history rendered and its stored agent, model and mode restored

#### Scenario: Skip flag needs an embedded server
- **WHEN** a user runs `cyber --dangerously-skip-permissions` without `--embedded`
- **THEN** the CLI prints `--dangerously-skip-permissions needs --embedded` and exits with code 2 without rendering the UI

### Requirement: First-run setup
(P0) The system SHALL show a setup wizard on first launch when no model provider is available. The wizard SHALL let the user connect a provider (API key, OAuth, environment variable detection, or a local server such as Ollama), pick a default model, and optionally run `cyber login` to sign in to a Cyber Account. Skipping the account step SHALL leave every local feature usable.

#### Scenario: Local model without an account
- **WHEN** a new user selects `Ollama` in the wizard and skips the Cyber Account step
- **THEN** the TUI saves the provider, selects the first detected local model as default, and opens the home screen with no login prompt

#### Scenario: Environment key detected
- **WHEN** `ANTHROPIC_API_KEY` is set on first launch
- **THEN** the wizard pre-selects the Anthropic provider as connected via environment and only asks the user to confirm the default model

### Requirement: Prompt editor
(P0) The system SHALL provide a multiline composer with these behaviors:
- Enter submits; Shift+Enter and Ctrl+J insert a newline.
- Up/Down move through persisted prompt history (per project, last 500 entries in `~/.local/state/cyber/history.jsonl`).
- Ctrl+G opens the current draft in `$VISUAL`/`$EDITOR` and replaces the draft with the saved content on exit.
- The draft is persisted when the TUI exits and restored on the next launch for the same Session.

#### Scenario: External editor round trip
- **WHEN** the user presses Ctrl+G with draft `fix the bug`, edits it to `fix the login bug` and saves
- **THEN** the composer contains `fix the login bug` and nothing is submitted

#### Scenario: Draft survives restart
- **WHEN** the user quits with an unsent draft and later runs `cyber -c`
- **THEN** the composer shows the unsent draft

### Requirement: Large paste and image input
(P0) The system SHALL summarize pasted text longer than 1,000 characters or 25 lines as a placeholder chip `[Pasted text #N +L lines]` whose full content is sent on submit, unless `tui.paste_summary` is `false`. The composer SHALL accept images (PNG, JPEG, GIF, WebP) and PDFs from:

- **clipboard paste:** the `composer.paste_image` binding reads the system clipboard, because terminals do not send image bytes. It SHALL use a fixed argument vector and no shell.
- **drag-and-drop:** a bracketed paste whose text is only paths of existing image or PDF files. Paths MAY be quoted, backslash-escaped or `file://` URLs.
- **`@path` mentions:** an image or PDF picked from the `@` menu or typed as a token.

The composer SHALL show images as `[Image #N]` chips and PDFs as `[PDF #N]` chips.

Chips SHALL behave as follows:

- **Numbering:** chips are numbered per draft.
- **Deletion:** a chip is deleted as one unit.
- **Drafts:** a draft SHALL keep its chips.
- **Submit:** text chips are expanded into the text part. Each media chip keeps its label in the text and adds one attachment part (`server-api` → Prompt attachment parts):
  - a server-read `file` part for a path, when the server runs on the TUI's machine,
  - inline bytes for clipboard images, and for paths when the TUI is attached to a peer,
  - `attachment` for chips restored by rewind.
- **Commands:** a `/` command, `!` shell or `&` background line also gets its text chips expanded. Media cannot go to these: a line that names an image or PDF chip SHALL be refused with a toast, keeping the draft and its chips.

Attachments larger than `attachments.max_bytes` (default 20 MiB) SHALL be rejected with the toast `Attachment exceeds 20 MiB` and not attached. A prompt the server refuses (size, unsupported type, missing model input, permission) SHALL keep the draft and its chips and show the server's message. The transcript SHALL show media parts as `[Image: <name>]` or `[PDF: <name>]`. Terminal image rendering is not provided.

#### Scenario: Large paste becomes a chip
- **WHEN** the user pastes 300 lines of log output
- **THEN** the composer shows `[Pasted text #1 +300 lines]` and the submitted message contains the full 300 lines

#### Scenario: Paste summary disabled
- **WHEN** `tui.paste_summary` is `false` and the user pastes 300 lines
- **THEN** the 300 lines are inserted into the composer as text

#### Scenario: Oversized image rejected
- **WHEN** the user drags in a 25 MiB PNG
- **THEN** the TUI shows `Attachment exceeds 20 MiB` and does not attach it

#### Scenario: Clipboard screenshot
- **WHEN** the clipboard holds a PNG screenshot and the user presses Ctrl+V in the composer
- **THEN** the composer shows `[Image #1]`, and the submitted prompt carries the image as an attachment part

#### Scenario: Empty clipboard
- **WHEN** the clipboard holds no image and the user presses Ctrl+V
- **THEN** the TUI shows `No image on the clipboard` and the composer is unchanged

#### Scenario: Dropped file path
- **WHEN** the terminal pastes `'/Users/me/Desktop/Screen Shot.png'` after a drag-and-drop
- **THEN** the composer shows `[Image #1]` instead of the path, and submitting sends a `file` part for that path

#### Scenario: PDF mention
- **WHEN** the user picks `docs/spec.pdf` from the `@` menu and submits `summarize [PDF #1]`
- **THEN** the prompt carries the PDF as an attachment part

#### Scenario: Chip deleted as a unit
- **WHEN** the cursor is right after `[Image #1]` and the user presses Backspace
- **THEN** the whole chip and its image are removed

#### Scenario: Model without image input
- **WHEN** the Session's model has no image input and the user submits a prompt with `[Image #1]`
- **THEN** the TUI shows the server's `has no image input` message and the draft keeps the chip

### Requirement: Mentions and fuzzy search
(P0) The system SHALL open an autocomplete menu when `@` is typed at a word boundary. The menu SHALL offer, ranked by fuzzy match:
- project files and directories (via `GET /api/v1/fs/find`, limit 20)
- non-hidden subagents
- MCP resources as `@server:uri`

A file mention SHALL accept a line range suffix `#L10` or `#L10-40`, and only that range SHALL be attached.

#### Scenario: File range mention
- **WHEN** the user selects `src/auth.rs` and types `#L10-40`
- **THEN** the submitted message carries a file part for `src/auth.rs` lines 10 through 40 only

#### Scenario: Agent mention invokes a subagent
- **WHEN** the user submits `@explore find all callers of parse_token`
- **THEN** the message is routed to the `explore` subagent as described in `agents-subagents`, without an `agent` permission prompt

### Requirement: Shell, slash and memory input modes
(P0) The system SHALL treat input starting with `!` as a user shell command. It SHALL run in the Location with the configured shell, and its output SHALL be recorded in the Session as user-provided context. Input starting with `/` SHALL open slash-command autocomplete over built-in TUI commands, custom commands, skills and MCP prompts (fuzzy, maximum 12 results). (P1) In the slash-command menu, Tab and Enter SHALL accept the selected entry, except that Enter SHALL run the command when the typed name already equals the selected entry. (P1) Input starting with `#` SHALL offer to save the remaining text as a memory entry in project or user scope, as defined in `memory`.

#### Scenario: Shell mode output in context
- **WHEN** the user submits `!cargo test 2>&1 | tail -20`
- **THEN** the command runs in the Location and the output appears as a shell block the model can see on the next Turn

#### Scenario: Enter runs a fully typed command
- **WHEN** the user types `/rename`, the menu selects `rename`, and the user presses Enter
- **THEN** `/rename` runs at once and the rename dialog opens, while Enter after `/rena` only completes the composer to `/rename `

#### Scenario: Memory quick-add
- **WHEN** the user submits `# always use pnpm, never npm`
- **THEN** the TUI asks `Save to: project / user` and, on `project`, appends the entry to the project memory

### Requirement: Steer and queue while running
(P0) While a Drain is running, the system SHALL support two ways to send a message:
- **Enter** admits the message with `delivery: steer`, applied at the next Safe Boundary.
- **Tab or Alt+Enter** admits it with `delivery: queue`, promoted when the Session would otherwise go idle.

Queued messages SHALL be listed above the composer in FIFO order. Each SHALL be editable or removable until promoted, using the take-back operation from `session-runtime`.

#### Scenario: Steer mid-turn
- **WHEN** the agent is running tests and the user types `skip the e2e suite` and presses Enter
- **THEN** the message is admitted as `steer` and appears in history right after the current Turn's tool results

#### Scenario: Remove a queued message
- **WHEN** the user queues `now update the changelog` and then deletes it from the queue list before the Drain goes idle
- **THEN** the admitted prompt is withdrawn and is never promoted to the model

### Requirement: Interrupt and rewind shortcut
(P0) The system SHALL interrupt the running Drain when Esc is pressed once, keeping all inbox rows. (P1) Pressing Esc twice within 800 ms while idle SHALL open the rewind menu.

#### Scenario: Interrupt preserves queued input
- **WHEN** the user presses Esc during a Turn while two messages are queued
- **THEN** the Drain stops, unsettled tools are shown as `interrupted`, and both queued messages remain in the queue list

#### Scenario: Double Esc opens rewind
- **WHEN** the Session is idle and the user presses Esc twice within 800 ms
- **THEN** the rewind menu opens with the user messages of the Session listed newest first

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

### Requirement: Permission mode indicator and cycling
(P1) The system SHALL always show the Session's permission mode in the footer. Shift+Tab SHALL cycle `default → accept-edits → plan → auto → default` as defined by `permissions-modes`. `bypass` and `dont-ask` SHALL be selectable only from `/mode <name>`, flags or config, and `bypass` only when it is not disabled by org policy, with an explicit confirmation. Mode changes SHALL be recorded as durable Session events.

#### Scenario: Cycle into plan mode
- **WHEN** the Session is in `accept-edits` and the user presses Shift+Tab
- **THEN** the mode becomes `plan`, the footer shows `⏸ plan` and the next Turn uses plan-mode rules

#### Scenario: Bypass blocked by policy
- **WHEN** org policy lists `bypass` in `modes.disable` and the user runs `/mode bypass`
- **THEN** the TUI shows `bypass mode is disabled by your organization` and the mode is unchanged

### Requirement: Model, variant and agent selection
(P0) The system SHALL provide a model picker (`/model`, Ctrl+X M) listing available models grouped by provider, with Favorites and Recents sections at the top. It SHALL be persisted in `~/.local/state/cyber/model.json` (maximum 10 recents). Ctrl+T in the picker SHALL cycle reasoning variants. Tab SHALL cycle primary, non-hidden agents. Selecting a model or agent SHALL call the Session switch operations, so the change applies at the next Turn.

#### Scenario: Unavailable model rejected
- **WHEN** a favorite model's provider has no credentials
- **THEN** the picker shows it dimmed with `not connected`, and selecting it opens the connect dialog instead of switching

#### Scenario: Agent cycling
- **WHEN** the user presses Tab in the composer with primary agents `build` and `docs` available
- **THEN** the active agent switches from `build` to `docs` and the footer shows the agent's color and name

### Requirement: Session picker and actions
(P0) The system SHALL provide a Session picker (`/resume`, Ctrl+X L). It SHALL list root Sessions of the Location newest first, with fuzzy search over name and title and a preview of the last message. Child and forked Sessions SHALL show as an expandable tree. From the picker or the command palette the user SHALL be able to rename, fork (from the latest message or a chosen one), archive, delete (with confirmation), and export a Session. `/rename <title>` SHALL rename the current Session at once, and `/rename` without a title SHALL open a dialog prefilled with the current title (Enter saves, Esc cancels). `/rename` SHALL be listed in `/help` and in the command palette as **Rename session**, with the binding action `session_rename` (unbound by default). The header, the sidebar and the terminal window title SHALL show a new title when `session.renamed.1` or `session.title.generated.1` arrives, whichever client renamed it.

#### Scenario: Fork from a message
- **WHEN** the user opens `/fork` and chooses the third user message
- **THEN** a new Session is created containing the messages before that point, and the TUI switches to it with the chosen message prefilled in the composer

#### Scenario: Delete requires confirmation
- **WHEN** the user selects Delete on a Session with two child Sessions
- **THEN** the TUI asks `Delete session and 2 child sessions?` and deletes nothing until confirmed

#### Scenario: Rename the current Session
- **WHEN** the user runs `/rename Auth refactor`
- **THEN** the Session is renamed and the header and sidebar show `Auth refactor` once `session.renamed.1` arrives

### Requirement: Rewind UI
(P1) The system SHALL provide `/rewind`, also opened by double Esc. It lists user messages; for the chosen one the user picks:
- **Code and conversation**
- **Conversation only**
- **Code only**

The TUI SHALL show the file diff summary (files, +/−) that the choice will apply, call the stage/commit revert operations from `snapshots-checkpoints`, and restore the chosen message's text and attachments into the composer. `/undo` and `/redo` SHALL step one user message backward or forward while a revert is staged.

#### Scenario: Code-only rewind
- **WHEN** the user rewinds `Code only` to message 4
- **THEN** files are restored to their state before message 4's Turns, while the conversation history stays intact and gains a marker noting the code rewind

#### Scenario: Redo after undo
- **WHEN** the user runs `/undo` and then `/redo` before sending anything
- **THEN** the staged revert is cleared and files and messages return to their pre-undo state

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

### Requirement: Agent view
(P2) The system SHALL provide an agent view, opened by `cyber agents` or Ctrl+A. It SHALL list every Session the server runs, across Locations, in three groups: **Needs input** (pending permission or question), **Working** (Drain active), and **Completed** (idle since the last view). Each row SHALL show name, Location, agent, model, the latest status line or question, cost and elapsed time. A **New task** row SHALL dispatch a background Session from a typed prompt, with optional `--worktree`. Enter SHALL attach to a row; Ctrl+A SHALL detach back to the view without interrupting the Session.

#### Scenario: Dispatch a background task
- **WHEN** the user types `fix flaky test in auth_spec` in the New task row and presses Enter
- **THEN** a new background Session is created in an isolated worktree and appears under Working

#### Scenario: Row needing input
- **WHEN** a background Session raises a permission request
- **THEN** its row moves to Needs input, shows the request title, and the attention notification fires

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

### Requirement: Goal and loop indicators
(P2) The system SHALL show a goal panel (`/goal` with no arguments) with the active Goal, the goal queue, the last evaluator verdict and its reason, Turns and cost used against the goal budget, and actions for edit, pause, resume, clear, and reorder of the queue. While a Goal is active, the footer SHALL show `◎ goal` with a truncated condition. Each active Loop SHALL show `↻ <interval>` with its next run time in the footer and in `/tasks`.

#### Scenario: Evaluator verdict shown
- **WHEN** the evaluator returns `not met: 3 tests still failing in api/`
- **THEN** the goal panel shows that reason and the footer goal indicator stays active

### Requirement: Status line
(P1) The system SHALL render a configurable status line as the last row of the screen, below the composer, from `tui.statusline.fields`. Available fields: `model`, `variant`, `agent`, `mode`, `context` (tokens and percent of the context window used, as `29.5K (22%)`), `cost` (`$0.00`, `—` when unpriced, `≥ $x` when partly unpriced), `branch`, `worktree`, `goal`, `loops`, `remote`, `cwd`. Default fields: `cwd, context, cost`, since the composer info line shows the agent, model and Mode, and the sidebar shows the branch. `cwd` SHALL be left-aligned, show the home directory as `~` as the sidebar does, and be shortened in the middle to fit. The other fields SHALL be right-aligned, separated by ` · `, and followed by the key hint `<command_palette key> commands` (default `ctrl+p commands`), which reflects the configured binding. A toast SHALL replace the left side for 6 seconds. When `tui.statusline.command` is set, the system SHALL run that command at most once every 300 ms with Session state as JSON on stdin, and render the first line of its stdout in place of the fields.

#### Scenario: Custom status command
- **WHEN** `tui.statusline.command` is `~/.config/cyber/status.sh`
- **THEN** the script receives JSON including `session_id`, `model`, `mode`, `context_used_pct` and `cost_usd`, and its first stdout line is shown in the footer

#### Scenario: Context warning
- **WHEN** context usage exceeds 85%
- **THEN** the `context` field is rendered in the warning color

#### Scenario: Default footer
- **WHEN** a Session in `/Users/leo/work/app`, with home `/Users/leo`, has used 29,470 of 131,072 context tokens and $0.00
- **THEN** the footer shows `~/work/app` on the left and `29.5K (22%) · $0.00  ctrl+p commands` on the right

### Requirement: Attention notifications
(P0) The system SHALL notify the user when a Session needs input or finishes a Drain while the terminal is unfocused or the Session is in the background. Supported methods are terminal bell, OSC 9 / OSC 777 desktop notification, or the native OS notifier, selected by `tui.notifications` (default `auto`). (P3) When remote control is active, the notification SHALL also be sent as a Device push, as defined in `remote-control`.

#### Scenario: Unfocused completion
- **WHEN** a 10-minute task finishes while the terminal window is unfocused
- **THEN** a desktop notification `cyber: <session name> finished` is shown

### Requirement: Themes and appearance
(P0) The system SHALL ship at least 12 built-in themes, including `cyber`, `system`, `dracula`, `tokyonight`, `catppuccin`, `gruvbox`, `nord`, `one-dark`, `github-light` and `solarized-light`. It SHALL load custom themes from `themes/*.json` in global and project config directories. Since a project's files may be untrusted, a theme file over 64 KiB, a `tui.jsonc` over 256 KiB, and either one when it is not a regular file (a FIFO, or a link to a device) SHALL be skipped with a warning, without reading past the limit or waiting on the file. It SHALL follow the terminal's dark/light background when `tui.theme` is `system`. `/theme` SHALL preview themes live, and the choice SHALL be persisted to `~/.local/state/cyber/kv.json`. (P1) The theme SHALL reload on SIGUSR2 without a restart. (P1) Every theme SHALL resolve to the Theme tokens, and colors SHALL be rendered as Color capability and NO_COLOR defines.

#### Scenario: Custom theme override
- **WHEN** both global and project `themes/` contain `corp.json`
- **THEN** the project theme wins and is listed once as `corp`

### Requirement: Keybindings
(P0) The system SHALL merge `tui.keybinds` over the defaults, supporting:
- a leader key (default Ctrl+X) with a 2,000 ms timeout
- chord sequences such as `<leader>m`
- context-scoped bindings for `composer` (`history_previous`, `history_next`, `newline`, `editor`, `paste_image`), and (P1) `palette`, `sidebar`, `settings` and `explorer`; `messages`, `picker`, `agent_view` and `permission` have no rebindable actions yet and are reported as unknown contexts
- disabling a binding with `"none"`

(P0) `composer.paste_image` SHALL default to Ctrl+V, and to Alt+V on Windows, where the terminal takes Ctrl+V for text paste.

(P1) `tui.vim_mode: true` SHALL enable vim normal/insert modes in the composer, also toggled with `/vim`. (P1) The defaults SHALL include `command_palette` (Ctrl+P), `sidebar_toggle` (`<leader>b`), `sidebar_focus` (`<leader>e`), `settings` (`<leader>,`), `file_explorer` (`<leader>o`), `theme_list` (`<leader>s`) and `theme_cycle` (`<leader>t`). Top-level `tui.keybinds` entries SHALL bind global actions, and an object named after a context SHALL bind that context's actions. A binding SHALL never be shown in a hint, the help screen or the palette when it is disabled. A value that does not parse, an unknown action or an unknown context SHALL be reported and leave the default in place.

#### Scenario: Disable a default binding
- **WHEN** `tui.keybinds.composer.history_previous` is `"none"`
- **THEN** pressing Up in the composer moves the cursor and no longer recalls history

#### Scenario: Rebound palette updates the footer hint
- **WHEN** `tui.keybinds.command_palette` is `"ctrl+k"`
- **THEN** Ctrl+K opens the palette and the footer hint reads `ctrl+k commands`

#### Scenario: Rebound image paste
- **WHEN** `tui.keybinds.composer.paste_image` is `"ctrl+shift+i"` and the clipboard holds an image
- **THEN** Ctrl+Shift+I attaches it, and Ctrl+V no longer reads the clipboard

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

### Requirement: Accessibility
(P1) The system SHALL provide `tui.accessibility.screen_reader: true` (or `CYBER_SCREEN_READER=1`). This mode disables spinners and animations, renders streaming text in whole-sentence chunks, labels every status with words rather than glyphs, and forces inline rendering. `tui.accessibility.reduced_motion` SHALL disable animations independently. Every theme SHALL meet a 4.5:1 contrast ratio for body text against its background, and 3:1 for `text_muted` and for focus indicators. Color SHALL never be the only carrier of meaning: every status dot, meter and heatmap SHALL also have a word, a percentage or a glyph level. Every panel, tab, sidebar row and palette entry SHALL be reachable and operable with the keyboard alone, and the focused element SHALL be marked with reverse video or a `›` marker. In screen-reader mode the sidebar SHALL not be drawn, the Turn footer marker SHALL be emitted as `Turn finished: agent <agent>, model <model>, <duration>`, and the Stats heatmap SHALL be replaced by a sentence such as `Active 265 of 340 days; most active May 20; current streak 14 days`.

#### Scenario: Screen reader mode
- **WHEN** screen-reader mode is on and a tool fails
- **THEN** the TUI emits the text line `Tool bash failed: <error>` instead of a red glyph-only card

#### Scenario: Heatmap as text
- **WHEN** screen-reader mode is on and the Stats tab opens
- **THEN** activity is described in one sentence and no heatmap grid is printed

### Requirement: Update prompt
(P0) The system SHALL check for updates at most once per 24 hours, unless `autoupdate` is `false` or `CYBER_DISABLE_AUTOUPDATE` is set. When a newer version exists, it SHALL show a non-blocking toast `cyber vX.Y.Z available — /upgrade`. It SHALL never restart a running Session to apply an update.

#### Scenario: Update notice during a run
- **WHEN** an update is found while a Drain is running
- **THEN** the toast appears and the Drain continues uninterrupted

### Requirement: Remote control indicator and pairing
(P3) The system SHALL show a `⇄ remote` footer indicator, with the count of connected Devices, while the Session is exposed through the Relay. `/remote` SHALL toggle exposure, as defined in `remote-control`. When pairing a new Device it SHALL render a QR code and a short pairing code in the terminal. Messages sent from a remote Device SHALL appear in the transcript labelled with the Device name.

#### Scenario: Pair a phone
- **WHEN** the user runs `/remote` while signed in to a Cyber Account and then `cyber remote pair`
- **THEN** the TUI shows a QR code and `Pairing code: 7K4-Q9M`, and after the phone pairs the footer shows `⇄ remote (1)`

#### Scenario: Not signed in
- **WHEN** the user runs `/remote` without a Cyber Account
- **THEN** the TUI offers `Sign in with cyber login to use remote control` and changes nothing

### Requirement: Mod panes and voice input
(P4) The system SHALL render plugin-provided UI panes, bands and buttons through the mods surface defined in `plugins-marketplace`, sandboxed to the regions the mod declares. (P4) It SHALL support hold-to-talk voice dictation (default `<leader>v`) through a configured speech-to-text provider, inserting the transcript into the composer without auto-submitting.

#### Scenario: Dictation inserts text
- **WHEN** the user holds `<leader>v`, says `run the migration tests` and releases
- **THEN** the composer contains `run the migration tests` and nothing is sent until Enter

### Requirement: Reasoning display and effort command
(P1) `tui.reasoning` SHALL accept `collapsed` (default), `hidden` and `expanded`, and Ctrl+T SHALL still toggle per Session. `/effort <minimal|low|medium|high|xhigh|max>` SHALL switch the Session model's reasoning variant (equivalent to `/model <current>#<level>`), listing only variants the model supports and respecting the org ceiling `models.max_variant` (`org-policy`). The footer `variant` field SHALL reflect the change at the next Turn.

#### Scenario: Raise effort for a hard problem
- **WHEN** the user runs `/effort xhigh` on a model that supports it
- **THEN** the next Turn uses the `xhigh` variant and the footer shows it

#### Scenario: Effort capped by policy
- **WHEN** org policy sets `models.max_variant: "high"` and the user runs `/effort max`
- **THEN** the TUI shows `effort "max" exceeds org ceiling "high"` and keeps the current variant

### Requirement: Cross-session message surfaces
(P1) The TUI SHALL render a received cross-session message in the transcript as a block labelled with the sender's address, Session ID and time and marked untrusted; a `send_message` call with its target, outcome (and reason) and message ID; and an idle notice as one line with the target, time and status. The footer SHALL show `✉ <n> queued` and `✉ <n> held` while the Session has unpromoted or held messages. Held messages SHALL appear as cards above the composer with actions to release (`r`), drop (`d`) and view (Enter), using the inbox release and drop routes. `/messages` (also in the command palette, binding `messages_view`, unbound by default) SHALL list the Session's message history with time, direction, peer address, outcome, size and the first line of text, filterable by direction and held state, expandable to the full text, and SHALL allow releasing and dropping held messages and clearing a `non_interactive` refusal override. All of these SHALL be built from durable events and routes, so they are restored after reconnect.

#### Scenario: Review traffic
- **WHEN** the user opens `/messages` in a Session that sent one message and received two
- **THEN** three rows are shown with their directions, outcomes and timestamps

#### Scenario: Release from the card
- **WHEN** a message is held in the attached Session and the user presses `r` on its card
- **THEN** the message is released, the card disappears, the held count in the footer drops by one, and the message is promoted at the next Safe Boundary

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

With the mouse captured, a click SHALL select an entry and a second click on it SHALL run it as Enter does. A click outside the palette SHALL leave it open.

#### Scenario: Open settings from the palette
- **WHEN** the user presses Ctrl+P, types `usage` and presses Enter
- **THEN** the Settings panel opens on the Usage tab

#### Scenario: Run an entry by clicking
- **WHEN** the palette filters to `Settings: Usage` and the user clicks that entry twice
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

### Requirement: Machine stats
(P1) Below LSP, the sidebar SHALL show a collapsible `▼ Machine` section about the machine the server runs on, from `GET /api/v1/system`, with the OS label right-aligned in the header. It SHALL have:
- `CPU`: a busy-percent bar and `N cores · X% idle`. Before the server has two CPU readings, the bar is empty and the text reads `N cores · measuring`.
- `Mem`: a used-percent bar and `used / total GB · free`.
- `Disk`: a used-percent bar for the Location's volume and `used / total GB · free`.
- `load a b c` where the server reports a load average.

Each bar SHALL be one row of eighth-block glyphs (`▏▎▍▌▋▊▉█`) on the `border` track, colored `success` below 70%, `warning` from 70% to 90% and `error` from 90%. Under `NO_COLOR` the bars SHALL be `[#####.....]` text. The TUI SHALL read the route every 3 seconds, and only while the sidebar is drawn (docked or as the drawer), the terminal is focused and `tui.machine_stats` is `on` (default; editable in the Config tab). A failed read SHALL keep the last sample, show `—` before the first one and raise no toast; a 404 or 503 SHALL omit the section, except that when `/health` reports another build than the TUI's (or none), the section SHALL show `needs a newer server · cyber service restart` in `warning`. In that case the sidebar SHALL also show `⚠ older server build · cyber service restart` under the Session title, and panels that read newer routes (the file explorer, the Usage and Stats tabs) SHALL show the same `needs a newer server` text for an undeclared 404. The Status tab SHALL show the same data as one `Machine:` sentence, which is also the screen-reader text, for example `CPU 18 percent busy, 12 cores; memory 71 percent used, 4.6 GB free; disk 93 percent used, 15 GB free`.

#### Scenario: Machine bars
- **WHEN** the sidebar is docked and the server reports 82% CPU idle on 12 cores, 71% of memory and 93% of the Location's disk in use
- **THEN** the Machine section shows a green CPU bar at 18% with `12 cores · 82% idle`, an amber memory bar at 71% and a red disk bar at 93%

#### Scenario: No polling while hidden
- **WHEN** the sidebar is hidden, the terminal loses focus or `tui.machine_stats` is `off`
- **THEN** the TUI makes no `GET /system` request until the sidebar is drawn again on a focused terminal

#### Scenario: First sample
- **WHEN** the server has only one CPU reading
- **THEN** the CPU row shows an empty bar and `measuring`, never an invented percentage

#### Scenario: Older server without the route
- **WHEN** the TUI was rebuilt and still talks to a server of the previous build that has no `GET /system`
- **THEN** the Machine section shows `needs a newer server · cyber service restart` instead of `—`, and the sidebar shows the older-build warning

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

### Requirement: Settings Status tab
(P1) The Status tab SHALL show, one `label: value` row each:
- client version and server version, in `warning` with `restart the service` when the versions or the build identities from `/health` differ (the server version then reads `<version> (older build)`)
- Session title and ID, Location directory, and git branch with a dirty marker
- agent, model with provider and variant, Mode, and context window use as a 30-column meter with tokens and percent
- the auto-compaction threshold
- a summary of MCP servers (connected out of configured, failed names) and of language servers

It SHALL include sandbox state and Cyber Account state only when the server reports them. It SHALL never show a placeholder for an unreported value.

#### Scenario: Version mismatch
- **WHEN** the TUI is 0.9.1 and the server reports 0.9.0
- **THEN** the version row shows both versions in the warning color with `restart the service`

#### Scenario: Same version, older build
- **WHEN** the TUI and the server both report 0.1.0 but `/health` reports another build, or no build
- **THEN** the server version reads `0.1.0 (older build)` in the warning color with `restart the service`

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

### Requirement: Nyan progress indicator
(P1) While the Session is working (a Turn, compaction, a running workflow the user started, or a long tool call), the progress line above the composer SHALL show the activity indicator selected by `tui.spinner`: `nyan` (default), `nyan-tall`, `braille`, `dots` or `off`.

`nyan` SHALL be one row: a rainbow trail, then the cat `[,,_,,]:3` in a pink accent with a tail glyph that alternates `~ - ~ =` per frame and a sparkle `✦` in the cell ahead of it on half of the frames, then the activity label and elapsed time. The trail SHALL be a smooth continuous rainbow: each cell's hue SHALL step a fixed amount along the trail and SHALL move with the frame so the colors flow toward the cat. Each cell SHALL be a `▀` or `▄` half block on the background, chosen by a sine wave of the cell position minus the frame, so the trail ripples. The tail SHALL fade toward the background, the cell next to the cat being the brightest. `nyan-tall` SHALL use 4 rows: six band half-rows, each a hue step from the column's flowing hue, lowered by half a row on the wave's troughs, and the 4-row cat art. It SHALL fall back to `nyan` when the terminal has fewer than 30 rows.

When the progress is unknown, the cat SHALL run across the bar like a comet, about one and a half cells per frame, its trail growing behind it up to two thirds of the bar, and SHALL wrap back to the start. The label SHALL say what the Turn is doing: `Thinking` while the model streams or reasons, and `Running <tool> <summary>` while a tool runs, followed by the elapsed time and the interrupt hint. Compaction SHALL show the label `compacting` in `warning` with its elapsed time, as unknown progress while the server reports no fraction. When progress is known (workflow agents done over total, or byte progress of a download or upload), the trail length SHALL be proportional to the fraction and keep flowing and rippling, the remainder SHALL be drawn as `·` in `text_subtle`, and the label SHALL show the count and percent.

While a permission request or question waits on the user, nothing SHALL animate. The indicator SHALL show the napping cat `[,,_,,]:3 zZ`, then `Waiting for approval` (or `Waiting for your answer`) in `warning`, what is asked about, the Turn's elapsed time, and for a permission the `y allow · n reject` hints.

Frames SHALL advance at no more than 12 per second, only while working, with nothing waiting on the user. The schedule and its check SHALL use one clock reading, so a frame is never due later than the moment it is checked. Rendering SHALL be skipped when the terminal reports that it lost focus (it counts as focused until it says otherwise) or when the indicator is off screen. With 16 colors the trail SHALL map to the nearest ANSI colors. Under `NO_COLOR` the trail SHALL be `=-` text and the cat SHALL still move, and with reduced motion the indicator SHALL be the static `[,,_,,]:3` with no animation. Screen readers SHALL receive `Working, <n> seconds` at most every 10 seconds and the label changes, never the trail glyphs.

#### Scenario: Unknown progress
- **WHEN** a Turn has been running for 12 seconds with `tui.spinner` at the default
- **THEN** the progress line shows the cat running with a flowing, rippling rainbow trail, `Thinking · 12.4s` and the interrupt hint, and the trail's cells change from one frame to the next

#### Scenario: A tool runs
- **WHEN** the Turn runs `bash cargo test`
- **THEN** the label reads `Running bash cargo test · <elapsed> · esc interrupt`

#### Scenario: Waiting for approval
- **WHEN** a bash permission request waits on the user during a Turn
- **THEN** the indicator shows `[,,_,,]:3 zZ  Waiting for approval · bash <command> · <elapsed> · y allow · n reject` and no animation frame is scheduled until it is answered

#### Scenario: Workflow progress
- **WHEN** a workflow run started from this Session has 7 of 10 agents done
- **THEN** the trail covers 70% of its width and the label reads `<name> 7/10 agents · 70%`

#### Scenario: Reduced motion
- **WHEN** reduced motion is on
- **THEN** the indicator is static and no animation frames are scheduled

### Requirement: Provider login commands
(P1) The TUI SHALL provide `/login [provider]` and `/logout [provider]`, backed by the server's provider login API:

- **Listing:** a bare `/login` SHALL open a picker of the subscriptions that support login (today `openai`, which is a ChatGPT Plus or Pro plan).
- **Login dialog:** `/login openai` (alias `chatgpt`) SHALL start an attempt and open a dialog. The dialog SHALL:
  - show the authorization URL, copied to the clipboard with Ctrl+Y,
  - say whether the server is waiting for the browser,
  - accept a pasted redirect URL submitted with Enter,
  - cancel the attempt on Esc.
- **Polling:** while the dialog is open, the TUI SHALL poll the attempt once a second.
- **Success and failure:** on success the dialog SHALL close, show `Signed in to ChatGPT as <email> (<plan>)` and open the model picker. A refused paste or a failed request SHALL be shown in the dialog, which stays open so the user can paste again or cancel. An attempt that ends `failed` SHALL close the dialog and show its error.
- **Refusals:** `/login google` (alias `gemini`) SHALL be refused, saying that Google subscription login is not supported. Any other provider SHALL be refused with a hint to configure its API key.
- **Logout:** `/logout openai` SHALL sign out and report whether a login was removed.

#### Scenario: Sign in from the TUI
- **WHEN** the user submits `/login openai`, opens the shown URL and approves in the browser
- **THEN** the dialog closes, the TUI shows the signed-in account and plan, and the model picker lists the plan's `chatgpt/*` models

#### Scenario: Headless paste
- **WHEN** the dialog says the server is not waiting for the browser and the user pastes the redirect URL and presses Enter
- **THEN** the attempt completes the same way

#### Scenario: Google refused
- **WHEN** the user submits `/login google`
- **THEN** no attempt starts and the TUI says that Google subscription login is not supported

### Requirement: Workflow run indicators
(P2) While any Workflow Run of the attached Location is `running`, `queued` or `paused` with agents still settling, the TUI SHALL show three indicators.
- **Sidebar.** When the sidebar is shown, a Workflows section after Context SHALL list runs owned by the current Session first, then the other active runs of the Location, at most 5 rows and then `+N more`. Each row SHALL show the name, a known-progress nyan bar of 6 cells and `completed/total`, or `#<n> in queue` instead of the bar and count while the run is queued. Enter on a row SHALL open that run in the Workflow monitor.
- **Status line.** A `workflows` status line field SHALL show `⚙ <n> workflows` (`⚙ 1 workflow` for one), followed by `· <q> queued` when runs are queued, and SHALL be omitted when no run is active.
- **Progress line.** The Nyan progress indicator's known-progress mode SHALL be fed by the current Session's running runs. With one, it SHALL show that run's completed over total agents. With several, it SHALL show the sum of completed over the sum of totals with the label `<n> workflows <done>/<total> agents · <pct>%`.

All values SHALL come from `GET /api/v1/workflows/runs` and the `workflow.*` events, and SHALL be re-read when the event stream reconnects. The section, field and progress feed SHALL disappear when the last active run ends.

#### Scenario: Two runs in the sidebar
- **WHEN** the current Session owns running runs `audit` (31/100) and `review` (0/2) and the sidebar is docked
- **THEN** the Workflows section shows `audit` and then `review`, each with its bar and count, and the footer shows `⚙ 2 workflows`

#### Scenario: Queued run in the sidebar
- **WHEN** `max_running_runs` is 1, `review` is running and `audit` and `deploy` are queued behind it
- **THEN** the Workflows section shows `deploy` with `#2 in queue`, `audit` with `#1 in queue` and `review` with its bar and `0/1`

#### Scenario: Aggregate progress
- **WHEN** the current Session's two running runs have 30 of 100 and 12 of 30 agents done
- **THEN** the progress line's trail covers 32% of its width and reads `2 workflows 42/130 agents · 32%`

#### Scenario: Queued count in the footer
- **WHEN** 8 runs are running and 2 are queued in the Location
- **THEN** the footer shows `⚙ 10 workflows · 2 queued`

### Requirement: Workflow run notifications
(P2) When a Workflow Run reaches `completed`, `failed`, `budget_exceeded` or `stopped`, or is paused by a server restart, the TUI SHALL show a one-line notice. The notice SHALL be added to the owner Session's transcript when that Session is open, and SHALL appear as a transient toast otherwise. It SHALL read `✓ <name> completed · <n> agents · <cost> · <elapsed>`, `✗ <name> failed: <diagnostic kind and message>`, `$ <name> budget exceeded: <limit>`, `■ <name> stopped` or `⏸ <name> paused by restart`.

While the terminal is unfocused, the same text SHALL be delivered through `tui.notifications`, as "Attention notifications" defines. `tui.workflow_notifications` SHALL select which runs notify: `own` (default: runs owned by the current Session, and runs this TUI started, paused, resumed or stopped), `location` (every run of the Location) or `off`. Each attempt's end SHALL notify at most once per TUI, including across event-stream reconnects.

#### Scenario: Background run finishes
- **WHEN** a run started by the current Session's `workflow` tool completes while the terminal is unfocused
- **THEN** the transcript gains `✓ audit completed · 100 agents · $4.10 · 21m` and a desktop notification with the same text is shown

#### Scenario: Run of another Session
- **WHEN** a run started by `cyber workflows run` in another terminal fails and `tui.workflow_notifications` is `own`
- **THEN** no notice is shown, and the run's status is visible in `/workflows`

#### Scenario: Short run of another Session with location
- **WHEN** `tui.workflow_notifications` is `location` and another Session's run in this Location fails before the TUI's first read of it
- **THEN** the TUI still shows its `✗ <name> failed: …` toast, once

#### Scenario: No duplicate after reconnect
- **WHEN** the event stream drops and reconnects after a run completed and was notified
- **THEN** the resync does not show the notice again

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

### Requirement: Output style picker
(P0) `/output-style` and the palette action **Output style…** SHALL open the output style picker, an overlay that lists the styles from `GET /api/v1/config/output-styles` for the Session's Location. Each row SHALL show the name, description and origin (`built-in`, `user` or `project`), with `current` on the Session's effective style and `default` on the configured one. The listing's issues, such as a held project directory, SHALL be shown below the list. Typing SHALL fuzzy-filter names. Enter SHALL switch the current Session. `d` SHALL save the highlighted built-in or user style as the user's default and switch the Session. Esc SHALL close the picker without side effects. `/output-style <name>` SHALL switch without opening the picker, and an unknown or held name SHALL show the server's reason. After a switch the status footer SHALL show `Output style: <name> (next turn)` for 2 seconds. When the Session's selected or configured style is unavailable, the TUI SHALL show the reason once per Session. `/output-style` SHALL be listed in `/help` and in the command palette. A subagent thread SHALL refuse `/output-style`.

#### Scenario: Switch from the picker
- **WHEN** the user opens `/output-style`, selects `explanatory` and presses Enter
- **THEN** the Session's output style becomes `explanatory`, the footer shows `Output style: explanatory (next turn)`, and the next request's system prompt ends with that style

#### Scenario: Held project style
- **WHEN** the user opens the picker in an untrusted checkout that has `.cyber/output-styles/reviewer.md`
- **THEN** `reviewer` is not listed and the picker shows that project output styles are held until the checkout is trusted

### Requirement: Export command
(P1) The system SHALL provide `/export [html|md|json] [path]` to export the current Session with the same renderers as `cyber sessions export`. The default format SHALL be `html`. The default path SHALL be `<title-slug>-<short-id>.<ext>` in the Location, where the slug is lowercase ASCII with other characters collapsed to `-`, at most 48 characters. The TUI SHALL ask before overwriting an existing file, SHALL print the written path, and SHALL note that the export contains the full conversation. `/export` SHALL be listed in `/help` and in the command palette (**Export session**).

#### Scenario: Default HTML export
- **WHEN** the user runs `/export` in a Session titled `Auth refactor` with ID `ses_01M58P862K…`
- **THEN** `auth-refactor-01m58p86.html` is written in the Location and its path is shown

#### Scenario: Existing file
- **WHEN** the target file already exists
- **THEN** the TUI asks before overwriting it and writes nothing if the user declines

### Requirement: Sound alerts
(P1) The TUI SHALL play a short sound for each alert event:
- `input`: a permission or question request is waiting;
- `done`: the Session finished a Drain and is idle, or the Session sent a notification (`session.notification.sent`);
- `error`: a Turn failed or `session.error` arrived;
- `workflow`: a watched workflow run completed;
- `workflow_failed`: a watched workflow run ended any other way: failed, stopped, over its budget, or paused by a restart;
- `message`: a cross-session message was delivered to or held for the attached Session.

`tui.sounds.when` SHALL be `unfocused` (default: play only while the terminal reports it is unfocused) or `always`.

At most one sound SHALL play every 1.5 seconds. An alert of higher priority, in the order `input`, `error`, `workflow_failed`, `done`, `workflow`, `message`, SHALL replace a waiting lower sound or cut a playing lower one short, and other alerts inside the window SHALL be dropped.

Playing a sound SHALL never block drawing or key handling. Sounds SHALL be played by the platform's own player, run directly and never through a shell:
- `afplay` on macOS;
- `pw-play`, `paplay` or `aplay` on Linux;
- `System.Media.SoundPlayer` through PowerShell on Windows, with the file path passed in an environment variable.

Each player process SHALL have no access to the terminal. It SHALL be killed after 5 seconds, when a newer sound starts, and when the TUI exits, including on SIGHUP and SIGTERM. A player SHALL only be searched for in absolute `PATH` entries.

The system SHALL fall back to the terminal bell when:
- the pack is `bell`;
- no player is found, or the player cannot be started;
- `SSH_CONNECTION` or `SSH_TTY` is set and `tui.sounds.play_over_ssh` is not `true`.

A player that exits with an error SHALL ring the bell for that alert and SHALL be kept for later alerts.

When a sound plays, the bell that `tui.notifications: auto` would write SHALL be left out. The desktop notification SHALL still be sent. At volume 0 no sound SHALL play, and the notification SHALL keep its bell.

`tui.sounds.enabled` SHALL default to `true`, and `tui.sounds.events.<event>: false` SHALL silence one event. The server and `cyber exec` SHALL never play sounds.

#### Scenario: Approval needed while away
- **WHEN** the terminal is unfocused, sounds use the defaults, and the attached Session asks for permission to run `npm test`
- **THEN** the `cyber` pack's `input` sound plays once through the detected player, an OSC 9 notification is written, and no BEL is written

#### Scenario: Burst coalesces
- **WHEN** the terminal is unfocused and a Session goes idle and a message arrives 200 ms apart
- **THEN** only the `done` sound plays

#### Scenario: Focused terminal stays quiet
- **WHEN** the terminal is focused, `tui.sounds.when` is `unfocused`, and a Turn finishes
- **THEN** no sound plays

#### Scenario: SSH session uses the bell
- **WHEN** `SSH_CONNECTION` is set, `play_over_ssh` is unset, and an `error` alert fires while unfocused
- **THEN** no player is started and a BEL is written to the terminal

#### Scenario: No player available
- **WHEN** no supported player is on `PATH` and a `done` alert fires while unfocused
- **THEN** a BEL is written, and one `info` log line names the reason

### Requirement: Sound packs
(P1) The system SHALL ship four packs:
- `cyber` (default): FM chimes;
- `soft`: sine tones;
- `retro`: square-wave arpeggios;
- `bell`: the terminal bell only.

The packs other than `bell` SHALL have a different sound for each of the six events. Their sounds SHALL be synthesized in code, with no audio files bundled, as mono 16-bit 44.1 kHz WAV. Each sound SHALL:
- last at most 0.9 seconds;
- peak at or below −1 dBFS;
- start and end below −40 dBFS over its first and last 2 ms;
- be byte-identical across runs for the same pack, event, volume and synthesizer version.

Rendered files SHALL be cached under the cache directory, keyed by pack, event, synthesizer version and volume, and written atomically.

`tui.sounds.volume` (0–100, default 60) SHALL scale the rendered samples. `tui.sounds.events.<event>` SHALL also accept:
- a pack name, to use that pack's sound for the event;
- an absolute or `~/` path to a regular file of at most 5 MiB with the extension `.wav`, `.aiff`, `.aif`, `.mp3`, `.ogg` or `.oga`.

A path SHALL be honoured only from the global `tui.jsonc`. A rejected or missing file, a file the detected player cannot decode (`aplay` and PowerShell take only `.wav`, `afplay` takes no Ogg), or a file the player fails to play SHALL fall back to the pack's sound with one warning.

A project `tui.jsonc` SHALL only be able to set `sounds.enabled: false`, lower `sounds.volume`, or set an event to `false`. Any other `sounds` value from a project SHALL be ignored with a warning.

#### Scenario: Deterministic, click-free sounds
- **WHEN** the `retro` `workflow` sound is rendered twice at volume 60
- **THEN** both files are byte-identical, the peak is at most −1 dBFS, and the first and last 2 ms are below −40 dBFS

#### Scenario: Project cannot pick a file
- **WHEN** a project `tui.jsonc` sets `sounds.events.input` to `/tmp/x.wav` and `sounds.enabled` to `true` while the global file has `enabled: false`
- **THEN** sounds stay off, the project values are ignored, and a warning names the ignored keys

#### Scenario: Custom file from global config
- **WHEN** the global `tui.jsonc` sets `sounds.events.done` to `~/sounds/done.wav`, a 40 KiB regular file
- **THEN** a `done` alert plays that file, and the other events keep the pack sounds

### Requirement: Sound settings command
(P1) `/sounds` (also in the command palette, binding `sounds_view`, unbound by default) SHALL open an overlay that:
- turns sounds on and off;
- picks the pack;
- changes the volume in steps of 10;
- sets `when`;
- turns each event on and off;
- previews each event's sound.

A preview SHALL play regardless of focus, `when` and the throttle. The overlay SHALL show the detected player, or the reason the bell fallback is used. Choices SHALL be saved to `kv.json`, where they override the global `tui.jsonc` unless that file was written after them (as `/theme`), and are overridden by the restrictions of a project `tui.jsonc`. Turning an event back on SHALL restore the pack or file the global `tui.jsonc` gives it. Every state SHALL be shown as a word, so the overlay is usable with `NO_COLOR` and in screen-reader mode.

#### Scenario: Pick a pack and preview
- **WHEN** the user opens `/sounds`, selects `retro`, and presses `p` on the `input` row
- **THEN** the `retro` `input` sound plays, and `retro` is still selected after the TUI restarts

#### Scenario: Fallback shown
- **WHEN** `/sounds` is opened over SSH without `play_over_ssh`
- **THEN** the overlay shows `bell only: SSH session`

### Requirement: Needs-input attention
(P0) When the attached Session gains a permission or question request, or one is routed to it, that this TUI has not alerted for, the TUI SHALL fire the attention notification of `tui.notifications` with the text `cyber: <session name> needs input`, together with the `input` sound alert. This SHALL happen once per request ID per TUI process, including when a reconnect or snapshot shows the same request again, and it SHALL follow the same focus rule as the `input` sound.

#### Scenario: Permission request notifies once
- **WHEN** the terminal is unfocused and a permission request appears, then the event stream reconnects and the request is still pending
- **THEN** exactly one notification `cyber: <session name> needs input` and one `input` sound are produced

### Requirement: Agent directory
(P1) `/agents` SHALL open a full-screen directory of every Session and agent the attached Session can reach by message. It SHALL also be in the command palette, with the binding `agents_view`, which is unbound by default. The directory SHALL read these sources:

- the messaging listing `GET /api/v1/messaging/sessions?from=<attached Session>`: subagents, teammates, other local Sessions, and direct-peer Sessions (`name@peer`, with `*@peer` for an offline peer);
- when project collaboration is enabled and the Session is bound to a project, the project's active hub agents and members from the collaboration participants route, except the signed-in member and the agent the attached Session is published as.

**Rows.** Rows SHALL be grouped as **This server**, **Peers** and **Project hub**. Each row SHALL show:

- the address to send to;
- a kind: `subagent`, `workflow agent`, `teammate`, `session`, `peer`, `hub agent` or `hub member`;
- a state: `idle`, `running`, `waiting_input` or `offline` (`online` or `offline` for hub agents; a hub member has no presence on the hub and shows `-`), as a glyph and a word;
- whether it can receive;
- its Location: the directory, the peer name, or `hub:<project>`.

**Filtering.** Typed characters SHALL filter the rows on address, name, kind and Location. Tab SHALL cycle a kind filter. Esc SHALL clear a non-empty filter before it closes the overlay.

**Selection and refresh.** The selection SHALL be kept by row identity across refreshes. The directory SHALL refresh on open, every 10 seconds while open (not while an editor is open), and after an action.

**Actions.** The selected row SHALL offer:

- **message:** Enter opens a compose editor. A listing row is sent through `POST /api/v1/messaging/send` from the attached Session. A hub row is sent as a direct message through the collaboration messages route. The outcome (`delivered`, `held` or `refused` with its reason) or the error SHALL be shown. An outcome SHALL close or annotate only the editor that submitted it; once that editor is closed, it SHALL be shown as a notice.
- **watch:** Ctrl+W, through `POST /api/v1/messaging/watch`.
- **open:** Ctrl+O switches the TUI to a local Session. On a peer row it shows the `cyber attach <session>@<peer>` command.
- **rename:** Ctrl+R, for local Sessions and subagents only, through the same rename used by `/rename` and the Session picker.
- **held messages:** Ctrl+G opens `/messages` filtered to held messages.

An action that does not apply to the row SHALL do nothing and say why. The directory SHALL use only the existing routes and SHALL NOT bypass inbound rules, rate limits or permissions.

**Untrusted text.** Every name, address, Location and hub text SHALL be stripped of control and bidi characters before display. A row whose address changes under that cleaning SHALL be sent to by its Session ID or not at all. Peer rows SHALL NOT be offered open-as-local or rename.

**Errors and empty states.** A source that fails SHALL show its error in its group without hiding the other sources:

- messaging disabled for the Location SHALL be shown as disabled (rows that cannot receive, or a disabled line when the listing itself is unavailable);
- a failed messaging refresh SHALL keep the last rows;
- a failed hub read SHALL clear the hub rows.

With no reachable rows, the directory SHALL say so and point to peer pairing and `/collab`. Under NO_COLOR, state SHALL be conveyed by glyph and word, and the selection by reverse video. A Session switch SHALL close and reset the directory.

#### Scenario: Local Sessions and a subagent
- **WHEN** the attached Session has one subagent named `tests`, and a Session titled `migration` runs in another directory on the same server, and the user runs `/agents`
- **THEN** **This server** lists `tests` as `subagent` and `migration` as `session` with its state and directory, and the attached Session itself is not listed

#### Scenario: Offline peer
- **WHEN** a configured peer `nas` cannot be reached and the user opens `/agents`
- **THEN** **Peers** shows `*@nas` as `peer`, `offline`, `closed`, and pressing Enter on it shows that it cannot receive instead of opening the editor

#### Scenario: Message from the directory
- **WHEN** the user types `migr`, presses Enter on `migration`, writes `schema is merged` and presses Enter
- **THEN** the message is sent from the attached Session to `migration`, the toast shows `delivered` and the message ID, and the filter still shows only `migration`

#### Scenario: Rename a local Session
- **WHEN** the user selects an untitled local Session listed by its `ses_` ID, presses Ctrl+R and enters `api-server`
- **THEN** the Session is renamed, and after the refresh its row's address is `api-server`

#### Scenario: Hub agent
- **WHEN** collaboration is enabled, the Session is bound to project `web`, and the hub lists an online agent `reviewer`
- **THEN** **Project hub** shows `reviewer` as `hub agent`, `online`, at `hub:web`; Enter sends a direct hub message to it; and Ctrl+W and Ctrl+R explain that they do not apply

#### Scenario: Crafted peer name
- **WHEN** a peer lists a Session whose name contains a bidi override and an escape sequence
- **THEN** the row shows the name without those characters, and actions on it send to the Session ID rather than to the crafted address

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

### Requirement: Sidebar reachable Session overview
(P1) The right sidebar SHALL provide a collapsible, initially expanded Sessions and agents overview after Context. It SHALL show at most six reachable named entries total, grouped as local Sessions, team/child agents and remote Sessions/agents, with actual state, kind and concise provenance. It SHALL use supplied Session titles or advertised agent names/addresses, sanitize external text and truncate by terminal display width. The preview budget SHALL reserve an entry for each nonempty group before filling remaining slots, so local history cannot starve team/remote names. Overflow SHALL link to the existing agent directory. The current Session title SHALL remain in the existing header without a duplicate reachable row. Human hub members and offline peer wildcard records SHALL NOT be counted as discovered Sessions.

#### Scenario: Named local and remote entries
- **WHEN** the attached Session can reach a local Session, a teammate and a Session on a paired peer
- **THEN** the sidebar shows their supplied names, actual states and distinct local/team/peer provenance without opening an overlay or inventing a remote connection

#### Scenario: Bounded list and duplicate names
- **WHEN** more than six entries are reachable and two remote entries have the same title
- **THEN** the overview renders at most six named entries with truthful total/overflow counts, keeps different identities distinct and exposes their provenance and full details through the directory

#### Scenario: No reachable Session or unavailable source
- **WHEN** a source is loading, unavailable, disabled, empty or reports an offline peer without Session rows
- **THEN** the corresponding state is explicit and is not rendered or counted as a fabricated live Session

### Requirement: Shared directory refresh lifetime
(P1) Sidebar and agent-directory views SHALL use the same authorized source loaders and shared source data. Refresh demand SHALL be coalesced to one read per source per ten-second interval while an expanded visible overview or directory needs it, except an explicit manual refresh. A hidden/collapsed sidebar alone SHALL NOT keep polling. Overlay filters/editors SHALL NOT change the sidebar's unfiltered projection. Source replies SHALL be fenced to the current Session, Location, connection, generation and observed account context; observed hub authorization loss SHALL clear hub rows. Network failures SHALL NOT block UI input or independent native Stop.

#### Scenario: Both views need data
- **WHEN** the directory is opened while the sidebar has fresh data or a refresh in flight
- **THEN** it can use that data and coalesces the read rather than starting an independent roster loop

#### Scenario: Context switch while remote data is pending
- **WHEN** a late peer/hub reply arrives after a Session, server, Location or relevant account context changes
- **THEN** it cannot repopulate the new context with the old roster or authorize an action under the old identity

#### Scenario: Hub access revoked
- **WHEN** the shared source observes that the current hub membership is no longer authorized
- **THEN** hub entries disappear from both views without preserving a stale roster as live data

### Requirement: Sidebar discovery navigation and accessibility
(P1) The overview SHALL follow sidebar collapse/focus, keyboard and mouse conventions. The section header SHALL toggle collapse; a directory footer or named row SHALL open the existing agent directory, selecting by stable identity when still available. Displaying or activating an overview row SHALL NOT automatically send, watch, rename, create or attach a Session. Navigation SHALL NOT retarget a stale selected index after refresh. Dark, light, NO_COLOR and narrow/drawer rendering SHALL preserve readable labels and existing native-control warning/Stop priority.

#### Scenario: Row changes before activation
- **WHEN** entries reorder or the focused entry disappears between render and activation
- **THEN** navigation follows its stable identity or opens the refreshed directory without acting on another row that inherited the index

#### Scenario: Native control while browsing the roster
- **WHEN** native control is active or uncertain and the sidebar or a busy directory editor has focus
- **THEN** its warning remains visible and the existing independent Stop shortcut is processed before roster/editor keys

#### Scenario: Accessible compact rendering
- **WHEN** the terminal uses NO_COLOR or a narrow sidebar drawer
- **THEN** names and state/source labels remain understandable without color and overflow remains discoverable without hiding connection/footer information

### Requirement: Resume at usage limit reset
(P1) When a Turn of the attached Session fails with `quota_exceeded` and the message names the reset time without a `not waiting` note, the error notice SHALL end with `/resume-at-reset to continue then`. `/resume-at-reset` SHALL call `POST /api/v1/sessions/{id}/usage-limit/resume` and show either the planned resume time or the refusal. While the attached Session has a pending usage limit wait, the footer SHALL show `⏸ usage limit · resumes <local time>`, a toast SHALL announce when the wait starts and when it resumes or is abandoned, and the interrupt key SHALL cancel the wait as it interrupts a Turn.

#### Scenario: Schedule a resume
- **WHEN** a Turn fails with `usage limit reached; resets at 14:20` and the user runs `/resume-at-reset`
- **THEN** the footer shows `⏸ usage limit · resumes 14:2x` and the Turn runs again after the reset without further input

#### Scenario: Nothing to resume
- **WHEN** the user runs `/resume-at-reset` after a Turn that ended normally
- **THEN** a toast shows the server's refusal and nothing is scheduled
