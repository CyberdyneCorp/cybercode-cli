## ADDED Requirements

### Requirement: Prompt history search
(P1) The composer SHALL search the project's prompt history (the entries Up/Down browse) when the user presses `composer.history_search` (default Ctrl+R). The search SHALL work as follows:

- **Start:** the current draft is set aside and the query starts empty. While the search is open the composer shows the selected entry, and the info line is replaced by `search: <query>` followed by the position `N/M` (or `no match`) and the keys `ctrl+r older · ↓ newer · enter edit · esc cancel`.
- **Matching:** an entry matches when it contains the query as a substring. The match ignores case unless the query has an uppercase letter. Entries are ordered newest first, and an entry whose text equals a newer entry is listed once. The empty query matches every entry.
- **Typing:** printable keys append to the query and Backspace removes its last character; either one selects the newest match again. Every occurrence of the query in the shown entry SHALL be marked with the `selection` style (reverse video under `NO_COLOR`).
- **Moving:** Ctrl+R (the binding) and Up select the next older match, and Down the next newer one. Moving past either end keeps the current match.
- **Accept:** Enter, Tab, Left and Right close the search and leave the selected entry in the composer for editing, with the cursor at its end; nothing is submitted. When nothing matches, accepting restores the draft.
- **Cancel:** Esc, Ctrl+G and Ctrl+C close the search and restore the draft exactly, chips included.
- **Other keys** are ignored while the search is open. Switching Session SHALL close the search. Because the draft never leaves the composer, a prompt or overlay that takes the keys leaves the search open beneath it, and quitting saves the draft, not the shown entry.

With `composer.history_search` set to `"none"`, Ctrl+R does nothing in the composer. Search runs locally over the history already loaded; nothing is sent to the server.

#### Scenario: Find an older prompt
- **GIVEN** the project history holds `run the tests`, `fix the login bug` and `run the linter`, oldest first
- **WHEN** the user presses Ctrl+R and types `run`
- **THEN** the composer shows `run the linter` with `run` marked, and the info line reads `search: run  1/2`
- **WHEN** the user presses Ctrl+R again and then Enter
- **THEN** the composer contains `run the tests` for editing and nothing is submitted

#### Scenario: Cancel keeps the draft
- **WHEN** the user has the draft `half-written idea`, presses Ctrl+R, types `lint` and presses Esc
- **THEN** the composer contains `half-written idea` again

#### Scenario: No match
- **WHEN** the user presses Ctrl+R and types `zzz` and no entry contains it
- **THEN** the info line reads `search: zzz  no match`, and Enter restores the draft

#### Scenario: Smart case
- **GIVEN** the history holds `Deploy staging` and `deploy prod`
- **WHEN** the user searches `deploy`
- **THEN** both entries match; searching `Deploy` matches only `Deploy staging`

## MODIFIED Requirements

### Requirement: Prompt editor
(P0) The system SHALL provide a multiline composer with these behaviors:
- Enter submits; Shift+Enter and Ctrl+J insert a newline.
- Up/Down move through persisted prompt history (per project, last 500 entries in `~/.local/state/cyber/history.jsonl`).
- (P1) Ctrl+R (`composer.history_search`) searches that history; see Prompt history search.
- Ctrl+G opens the current draft in `$VISUAL`/`$EDITOR` and replaces the draft with the saved content on exit.
- The draft is persisted when the TUI exits and restored on the next launch for the same Session.

#### Scenario: External editor round trip
- **WHEN** the user presses Ctrl+G with draft `fix the bug`, edits it to `fix the login bug` and saves
- **THEN** the composer contains `fix the login bug` and nothing is submitted

#### Scenario: Draft survives restart
- **WHEN** the user quits with an unsent draft and later runs `cyber -c`
- **THEN** the composer shows the unsent draft

### Requirement: Keybindings
(P0) The system SHALL merge `tui.keybinds` over the defaults, supporting:
- a leader key (default Ctrl+X) with a 2,000 ms timeout
- chord sequences such as `<leader>m`
- context-scoped bindings for `composer` (`history_previous`, `history_next`, `history_search`, `newline`, `editor`, `paste_image`), and (P1) `palette`, `sidebar`, `settings` and `explorer`; `messages`, `picker`, `agent_view` and `permission` have no rebindable actions yet and are reported as unknown contexts
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
