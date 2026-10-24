## MODIFIED Requirements

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
