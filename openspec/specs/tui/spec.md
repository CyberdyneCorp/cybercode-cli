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

Piped stdin SHALL be prepended to `--prompt`. Invalid combinations SHALL fail before the UI renders, with exit code 2.

#### Scenario: Fork requires a source session
- **WHEN** a user runs `cyber --fork` without `--continue` or `--resume`
- **THEN** the CLI prints `--fork requires --continue or --resume` and exits with code 2 without rendering the UI

#### Scenario: Piped stdin becomes the prompt
- **WHEN** a user runs `git diff | cyber --prompt "review this"`
- **THEN** the first submitted message contains the diff text followed by `review this`

#### Scenario: Resume by name
- **WHEN** a user runs `cyber -r auth-refactor` and exactly one Session in the Location is named `auth-refactor`
- **THEN** the TUI opens that Session with its history rendered and its stored agent, model and mode restored

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
(P0) The system SHALL summarize pasted text longer than 1,000 characters or 25 lines as a placeholder chip `[Pasted text #N +L lines]` whose full content is sent on submit, unless `tui.paste_summary` is `false`. The composer SHALL accept images and PDFs from clipboard paste, drag-and-drop file paths, or `@path` mentions, showing them as `[Image #N]` chips. Attachments larger than 20 MiB SHALL be rejected with a toast.

#### Scenario: Large paste becomes a chip
- **WHEN** the user pastes 300 lines of log output
- **THEN** the composer shows `[Pasted text #1 +300 lines]` and the submitted message contains the full 300 lines

#### Scenario: Oversized image rejected
- **WHEN** the user drags in a 25 MiB PNG
- **THEN** the TUI shows `Attachment exceeds 20 MiB` and does not attach it

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
(P0) The system SHALL treat input starting with `!` as a user shell command. It SHALL run in the Location with the configured shell, and its output SHALL be recorded in the Session as user-provided context. Input starting with `/` SHALL open slash-command autocomplete over built-in TUI commands, custom commands, skills and MCP prompts (fuzzy, maximum 12 results). (P1) Input starting with `#` SHALL offer to save the remaining text as a memory entry in project or user scope, as defined in `memory`.

#### Scenario: Shell mode output in context
- **WHEN** the user submits `!cargo test 2>&1 | tail -20`
- **THEN** the command runs in the Location and the output appears as a shell block the model can see on the next Turn

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
- assistant text as Markdown with syntax-highlighted code blocks
- reasoning collapsed by default; Ctrl+T toggles it per Session
- each tool call as a collapsible card with title, status (`pending`, `running`, `completed`, `failed`, `interrupted`), duration and summary
- file edits as diffs, in `unified` or `split` style (`tui.diff_style`, default `auto`: split when the terminal is at least 160 columns wide)

`/timestamps` SHALL toggle per-message timestamps. Tool output over 30 lines SHALL be collapsed to its last 10 lines until expanded.

#### Scenario: Split diff on wide terminal
- **WHEN** an edit completes and the terminal is 200 columns wide with `diff_style` set to `auto`
- **THEN** the edit card renders a side-by-side diff

#### Scenario: Long bash output collapsed
- **WHEN** a bash tool returns 400 lines
- **THEN** the card shows the last 10 lines and `+390 lines (press o to expand)`

### Requirement: Permission prompt
(P0) The system SHALL show a blocking prompt for each pending permission request of the active Session tree. The prompt SHALL show the tool title, the resources, and a diff preview for edits. It SHALL offer:
- **Allow once**
- **Allow always**, followed by a confirmation step listing the exact save patterns and their scope (project or user)
- **Reject**, with an optional feedback message sent back to the model

Requests SHALL be shown one at a time in arrival order.

#### Scenario: Allow always confirmation
- **WHEN** bash requests `npm run test -- --watch=false` and the user picks Allow always
- **THEN** the TUI shows `Always allow: bash "npm run test *" (project)` and saves the approval only after the user confirms

#### Scenario: Reject with feedback
- **WHEN** the user rejects an edit and types `use the existing helper instead`
- **THEN** the reply is `reject` with that message, and the model receives it as a tool error instead of the Drain halting

### Requirement: Question prompt
(P0) The system SHALL render `question` tool requests as an inline form:
- a single-select question submits on selection
- multiple questions use tabs with a final review step
- questions that allow custom answers offer `Type your own answer`

Esc SHALL dismiss the request, which halts the Drain as defined in `session-runtime`.

#### Scenario: Multi-question tabs
- **WHEN** the agent asks two questions, `Database?` and `Auth method?`
- **THEN** the TUI shows two tabs plus a Review tab and sends both answers together on confirm

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
(P0) The system SHALL provide a Session picker (`/resume`, Ctrl+X L). It SHALL list root Sessions of the Location newest first, with fuzzy search over name and title and a preview of the last message. Child and forked Sessions SHALL show as an expandable tree. From the picker or the command palette the user SHALL be able to rename, fork (from the latest message or a chosen one), archive, delete (with confirmation), and export a Session.

#### Scenario: Fork from a message
- **WHEN** the user opens `/fork` and chooses the third user message
- **THEN** a new Session is created containing the messages before that point, and the TUI switches to it with the chosen message prefilled in the composer

#### Scenario: Delete requires confirmation
- **WHEN** the user selects Delete on a Session with two child Sessions
- **THEN** the TUI asks `Delete session and 2 child sessions?` and deletes nothing until confirmed

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

#### Scenario: Stop one background command
- **WHEN** the user selects a running `npm run dev` job and presses `x`
- **THEN** the job is terminated, its status becomes `cancelled`, and the model is notified at the next Safe Boundary

### Requirement: Subagent threads and side chat
(P1) The system SHALL provide `/agent` (alias `/subagents`) to switch the view to any running or finished subagent thread of the Session, show its transcript, and send it messages when it is resumable. (P1) `/btw <question>` (alias `/side`) SHALL open an ephemeral overlay chat. The side chat SHALL see the current Session context read-only, SHALL NOT add messages to the main Session, and SHALL be discarded on close unless the user chooses `Insert answer`.

#### Scenario: Side question does not pollute history
- **WHEN** the user asks `/btw what does the --frozen flag do?` while the main agent works
- **THEN** the answer appears in an overlay and the main Session history gains no message

### Requirement: Agent view
(P2) The system SHALL provide an agent view, opened by `cyber agents` or Ctrl+A. It SHALL list every Session the server runs, across Locations, in three groups: **Needs input** (pending permission or question), **Working** (Drain active), and **Completed** (idle since the last view). Each row SHALL show name, Location, agent, model, the latest status line or question, cost and elapsed time. A **New task** row SHALL dispatch a background Session from a typed prompt, with optional `--worktree`. Enter SHALL attach to a row; Ctrl+A SHALL detach back to the view without interrupting the Session.

#### Scenario: Dispatch a background task
- **WHEN** the user types `fix flaky test in auth_spec` in the New task row and presses Enter
- **THEN** a new background Session is created in an isolated worktree and appears under Working

#### Scenario: Row needing input
- **WHEN** a background Session raises a permission request
- **THEN** its row moves to Needs input, shows the request title, and the attention notification fires

### Requirement: Workflow monitor
(P2) The system SHALL provide `/workflows`. It lists running and finished Workflow Runs with name, phase, agents completed/total, tokens, cost against budget, and elapsed time. Keys SHALL be `p` pause/resume, `x` stop, `r` relaunch, Enter drill into a phase, and `t` open a single agent's transcript. A running Workflow Run SHALL show a compact progress line above the composer.

#### Scenario: Pause a run
- **WHEN** the user presses `p` on a running Workflow Run with 6 agents in flight
- **THEN** no new agents start, in-flight agents finish, and the run shows `paused (6 settling)` and then `paused`

#### Scenario: Budget display
- **WHEN** a run has spent $1.80 of a $5.00 budget
- **THEN** the monitor shows `$1.80 / $5.00` and colors it warning once spend exceeds 80%

### Requirement: Goal and loop indicators
(P2) The system SHALL show a goal panel (`/goal` with no arguments) with the active Goal, the goal queue, the last evaluator verdict and its reason, Turns and cost used against the goal budget, and actions for edit, pause, resume, clear, and reorder of the queue. While a Goal is active, the footer SHALL show `◎ goal` with a truncated condition. Each active Loop SHALL show `↻ <interval>` with its next run time in the footer and in `/tasks`.

#### Scenario: Evaluator verdict shown
- **WHEN** the evaluator returns `not met: 3 tests still failing in api/`
- **THEN** the goal panel shows that reason and the footer goal indicator stays active

### Requirement: Status line
(P1) The system SHALL render a configurable status line from `tui.statusline.fields`. Available fields: `model`, `variant`, `agent`, `mode`, `context` (percent of context window used), `cost`, `branch`, `worktree`, `goal`, `loops`, `remote`, `cwd`. Default fields: `model, mode, context, cost, branch`. When `tui.statusline.command` is set, the system SHALL run that command at most once every 300 ms with Session state as JSON on stdin, and render the first line of its stdout.

#### Scenario: Custom status command
- **WHEN** `tui.statusline.command` is `~/.config/cyber/status.sh`
- **THEN** the script receives JSON including `session_id`, `model`, `mode`, `context_used_pct` and `cost_usd`, and its first stdout line is shown in the footer

#### Scenario: Context warning
- **WHEN** context usage exceeds 85%
- **THEN** the `context` field is rendered in the warning color

### Requirement: Attention notifications
(P0) The system SHALL notify the user when a Session needs input or finishes a Drain while the terminal is unfocused or the Session is in the background. Supported methods are terminal bell, OSC 9 / OSC 777 desktop notification, or the native OS notifier, selected by `tui.notifications` (default `auto`). (P3) When remote control is active, the notification SHALL also be sent as a Device push, as defined in `remote-control`.

#### Scenario: Unfocused completion
- **WHEN** a 10-minute task finishes while the terminal window is unfocused
- **THEN** a desktop notification `cyber: <session name> finished` is shown

### Requirement: Themes and appearance
(P0) The system SHALL ship at least 12 built-in themes, including `cyber`, `system`, `dracula`, `tokyonight`, `catppuccin`, `gruvbox`, `nord`, `one-dark`, `github-light` and `solarized-light`. It SHALL load custom themes from `themes/*.json` in global and project config directories. It SHALL follow the terminal's dark/light background when `tui.theme` is `system`. `/theme` SHALL preview themes live, and the choice SHALL be persisted to `~/.local/state/cyber/kv.json`. (P1) The theme SHALL reload on SIGUSR2 without a restart.

#### Scenario: Custom theme override
- **WHEN** both global and project `themes/` contain `corp.json`
- **THEN** the project theme wins and is listed once as `corp`

### Requirement: Keybindings
(P0) The system SHALL merge `tui.keybinds` over the defaults, supporting:
- a leader key (default Ctrl+X) with a 2,000 ms timeout
- chord sequences such as `<leader>m`
- context-scoped bindings for `composer`, `messages`, `picker`, `agent_view` and `permission`
- disabling a binding with `"none"`

(P1) `tui.vim_mode: true` SHALL enable vim normal/insert modes in the composer, also toggled with `/vim`.

#### Scenario: Disable a default binding
- **WHEN** `tui.keybinds.composer.history_previous` is `"none"`
- **THEN** pressing Up in the composer moves the cursor and no longer recalls history

### Requirement: Rendering modes and terminal integration
(P0) The system SHALL support two rendering modes, selected by `tui.render` (default `fullscreen`, or `--inline`):
- **fullscreen**: alternate screen with mouse support and stable memory
- **inline**: append-only scrollback with a repaintable footer, like Codex and OpenCode mini

The system SHALL copy selections with OSC 52 when the native clipboard is unavailable. It SHALL set the terminal title to `cyber · <session name> · <status>` (configurable by `tui.title.fields`). On exit it SHALL print a summary with tokens, cost and the resume command `cyber -r <name|id>`.

#### Scenario: Exit summary
- **WHEN** the user quits a Session named `auth-refactor`
- **THEN** the terminal shows the total cost and tokens, and `Resume with: cyber -r auth-refactor`

#### Scenario: Clipboard over SSH
- **WHEN** the user copies a code block over SSH with no native clipboard available
- **THEN** the TUI emits an OSC 52 sequence so the local terminal receives the text

### Requirement: Accessibility
(P1) The system SHALL provide `tui.accessibility.screen_reader: true` (or `CYBER_SCREEN_READER=1`). This mode disables spinners and animations, renders streaming text in whole-sentence chunks, labels every status with words rather than glyphs, and forces inline rendering. `tui.accessibility.reduced_motion` SHALL disable animations independently. Every theme SHALL meet a 4.5:1 contrast ratio for body text against its background.

#### Scenario: Screen reader mode
- **WHEN** screen-reader mode is on and a tool fails
- **THEN** the TUI emits the text line `Tool bash failed: <error>` instead of a red glyph-only card

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
