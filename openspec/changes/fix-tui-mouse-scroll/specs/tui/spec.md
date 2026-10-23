## MODIFIED Requirements

### Requirement: Rendering modes and terminal integration
(P0) The system SHALL support two rendering modes, selected by `tui.render` (default `fullscreen`, or `--inline`):
- **fullscreen**: alternate screen with mouse support and stable memory. The TUI SHALL capture the mouse unless `tui.mouse` is `off`: a wheel notch SHALL scroll the transcript by 3 lines, or move Up/Down in a panel that scrolls or lists, and prompts and confirmations SHALL ignore the wheel. With `tui.mouse: off` the terminal keeps the mouse for its own selection and scrollback
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
