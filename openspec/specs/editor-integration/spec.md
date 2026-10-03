# editor-integration Specification

## Purpose
Editor integration brings Cyber Code into IDEs without forking its runtime. It offers an Agent Client Protocol (ACP) agent over stdio for Zed, JetBrains, Neovim and other ACP clients (from OpenCode v1), a first-party VS Code extension with inline diffs and plan review (from Claude Code), an IDE-context source that shares open files, selection and diagnostics (from Codex `/ide`), and deep links that open a Session in the right repository. Every integration is a client of the same local server and permission system.

## Requirements

### Requirement: ACP command and transport
(P1) The system SHALL provide `cyber acp [--cwd <dir>]`, which starts or attaches to a local server and speaks ACP as newline-delimited JSON-RPC 2.0 over stdin/stdout until stdin closes. Logs SHALL go only to the log file and never to stdout. Sessions created through ACP SHALL carry client identity `acp`.

#### Scenario: Process lifetime
- **WHEN** the editor closes the ACP process's stdin
- **THEN** the system interrupts running Drains owned by that connection and exits with code 0

### Requirement: ACP initialize and capabilities
(P1) The system SHALL answer `initialize` with ACP protocol version 1, agent info `{ name: "Cyber Code", version }`, and capabilities:
- `loadSession: true`
- prompt `image` and `embeddedContext`
- MCP `http` and `sse`
- session `list`, `resume`, `close` and `fork`

It SHALL advertise the auth method `cyber-login` for clients that support terminal auth.

#### Scenario: Capabilities advertised
- **WHEN** Zed sends `initialize`
- **THEN** the response declares `loadSession: true` and session capabilities including `fork`

### Requirement: ACP session lifecycle
(P1) The system SHALL map these ACP methods to Cyber Code Sessions:
- `session/new`: create a Session in `cwd` with the default agent, model and mode.
- `session/load`: replay full history as message, thought and tool updates.
- `session/resume`: no replay.
- `session/fork`
- `session/list`: root Sessions filtered by `cwd`, pages of 100, newest first.
- `session/close`
- `session/cancel`: interrupt the Drain.

Unknown session IDs SHALL fail with invalid params `session not found: <id>`.

#### Scenario: Load replays history
- **WHEN** the client calls `session/load` for a Session with 3 Turns
- **THEN** the system streams `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk` and tool call updates reproducing those Turns before responding

### Requirement: ACP config options
(P1) The system SHALL expose these session config options:
- `model`: values `provider/model`
- `mode`: values are the primary agents
- `permission_mode`: values `default`, `accept-edits`, `plan`, `auto` and `dont-ask`
- `effort`: present only when the model has variants

`session/set_config_option` SHALL validate each value and fail with invalid params for unknown values.

#### Scenario: Switch permission mode from the editor
- **WHEN** the client sets `permission_mode` to `accept-edits`
- **THEN** the Session mode changes, a durable mode-switch event is recorded, and a `config_option_update` is sent

### Requirement: ACP prompting and streaming
(P1) The system SHALL convert ACP content blocks into message parts:
- text → text
- images → file parts
- `resource_link` → file mention
- embedded resources → text prefixed with `[path:line]`

Prompts starting with `/` SHALL run the matching command or skill. The system SHALL stream text and reasoning deltas, and respond with stop reason `end_turn`, `cancelled`, `max_tokens` or `refusal`, plus a `usage_update` carrying context tokens, context limit and cost.

#### Scenario: Slash command over ACP
- **WHEN** the client sends `/review HEAD~1`
- **THEN** the `review` command runs with argument `HEAD~1` instead of being sent as plain text

### Requirement: ACP tool calls and permissions
(P1) The system SHALL report each tool call as `tool_call` (pending), then `tool_call_update` (`in_progress`, `completed` or `failed`). Kinds SHALL map as:

| Tools | Kind |
|---|---|
| bash | `execute` |
| read | `read` |
| edit, write, apply_patch | `edit` |
| glob, grep | `search` |
| webfetch | `fetch` |
| subagent, workflow | `think` |
| anything else | `other` |

Each permission request SHALL be forwarded as `session/request_permission` with options `allow_once`, `allow_always` and `reject`, one at a time per Session. Cancellation or a client without permission support SHALL count as `reject`.

#### Scenario: Edit permission with diff
- **WHEN** the agent proposes an edit in `ask` mode
- **THEN** the client receives a permission request containing the diff, and on `allow_once` the edit proceeds

### Requirement: ACP file write-through
(P1) The system SHALL, when the client advertises `fs.writeTextFile`, apply approved edits through `fs/write_text_file` so editor buffers stay authoritative. It SHALL also read unsaved buffer content through `fs/read_text_file` when the client advertises it. The snapshot and checkpoint records SHALL still capture the change.

#### Scenario: Unsaved buffer respected
- **WHEN** a file has unsaved changes in the editor and the agent reads it
- **THEN** the read returns the buffer content from the client, not the on-disk content

### Requirement: ACP client MCP servers
(P1) The system SHALL register MCP servers passed in `session/new`, `load`, `resume` or `fork` for that Session only. stdio servers SHALL map to local servers and http/sse servers to remote ones. Identical servers SHALL be deduplicated per Session, and a registration failure SHALL be reported as a warning without failing the Session.

#### Scenario: Editor-provided MCP server
- **WHEN** the client passes an stdio MCP server `docs` in `session/new`
- **THEN** its tools are available to that Session and not to other Sessions

### Requirement: VS Code extension
(P1) The system SHALL publish the VS Code extension `cyber-code.cyber` (also published to Open VSX for Cursor, Windsurf and VSCodium). It SHALL provide:
- a chat panel and an optional integrated-terminal launch (Cmd/Ctrl+Esc)
- session history and resume
- mode and model selectors

It SHALL connect to the user's background server, starting `cyber service start` if needed, through the public API.

#### Scenario: Panel attaches to the service
- **WHEN** the user opens the Cyber Code panel in VS Code
- **THEN** the extension ensures the background server is running and lists the workspace's Sessions

### Requirement: Inline diff review
(P1) The VS Code extension SHALL show pending and completed edits as native inline diffs, with per-hunk and per-file Accept and Reject actions. In `default` mode, Accept and Reject SHALL answer the pending permission request. In `accept-edits` mode, Reject SHALL stage a code-only revert of that file through `snapshots-checkpoints`.

#### Scenario: Reject a hunk
- **WHEN** the user rejects one hunk of a three-hunk edit awaiting approval
- **THEN** the permission reply is `reject` with feedback naming the rejected hunk, and the model receives it as a tool error

### Requirement: Selection mentions and plan review
(P1) The VS Code extension SHALL insert `@<workspace-relative path>#L<start>-<end>` for the current selection (Alt+K). It SHALL send the selection to the composer with a command. In `plan` mode it SHALL render the proposed plan as a reviewable document, and the user SHALL approve or request changes before the agent exits plan mode.

#### Scenario: Selection reference
- **WHEN** the user selects lines 12-30 of `src/app.ts` and presses Alt+K
- **THEN** the composer receives `@src/app.ts#L12-30`

#### Scenario: Approve a plan
- **WHEN** the agent calls `plan_exit` with a plan
- **THEN** the extension opens the plan with Approve and Request changes actions, and Approve switches the Session out of plan mode

### Requirement: IDE context source
(P1) The system SHALL accept IDE context from a connected IDE through `POST /api/v1/ide/context`: open files, active file, selection and error diagnostics. When `ide.share_context` is `true` (the default when connected), it SHALL expose that context to the model as a System Context source (`ide/context`), updated only at Safe Boundaries. `/ide` SHALL show the connected IDE and toggle context sharing.

#### Scenario: Diagnostics reach the model
- **WHEN** the IDE reports 2 errors in the active file and the next Turn starts
- **THEN** the model receives a Mid-Conversation System Message listing those diagnostics

#### Scenario: Context sharing off
- **WHEN** the user runs `/ide` and disables context sharing
- **THEN** subsequent Turns receive no IDE context and the source is removed at the next Safe Boundary

### Requirement: IDE detection and extension install
(P1) The system SHALL detect the hosting IDE from `TERM_PROGRAM` and IDE-specific environment variables (VS Code, Cursor, Windsurf, VSCodium, JetBrains terminals). It SHALL offer `cyber ide install`, which installs the matching extension through the IDE's CLI. It SHALL never install an extension automatically without the user running that command.

#### Scenario: Install in Cursor
- **WHEN** a user runs `cyber ide install` inside Cursor's terminal
- **THEN** the system runs `cursor --install-extension cyber-code.cyber` and reports the result

### Requirement: JetBrains plugin
(P4) The system SHALL provide a JetBrains plugin that connects to the local server with the same capabilities as the VS Code extension: chat tool window, inline diffs, selection mentions and IDE context. IDEs with native ACP support MAY use `cyber acp` instead.

#### Scenario: JetBrains via ACP
- **WHEN** a JetBrains IDE with ACP support is configured with command `cyber acp`
- **THEN** sessions, permissions and tool updates work as specified for ACP clients

### Requirement: Deep links
(P3) The system SHALL register the URL scheme `cyber://` on install where the OS allows it. It SHALL handle `cyber://open?repo=<owner/name>&cwd=<path>&prompt=<text>&mode=<mode>` by opening a terminal session in the matching local clone (resolved from known projects, or `cwd`). The prompt SHALL be prefilled but never submitted until the user confirms. Links whose `cwd` is not a known project SHALL show a confirmation naming the directory first.

#### Scenario: Runbook link
- **WHEN** a user clicks `cyber://open?repo=acme/api&prompt=Investigate%205xx%20spike`
- **THEN** a terminal opens `cyber` in the local `acme/api` clone with the prompt prefilled and not sent

#### Scenario: Unknown directory confirmation
- **WHEN** a link points to `cwd=/tmp/untrusted`, which is not a known project
- **THEN** the system asks `Open Cyber Code in /tmp/untrusted?` before launching
