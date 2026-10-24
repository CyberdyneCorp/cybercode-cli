## Context

`fix-tui-mouse-scroll` (PR #25) added `crates/cyber-tui/src/mouse.rs`:

- `App::on_mouse` scrolls the transcript by `WHEEL_LINES`.
- In a panel that `wheel_moves()` accepts, it turns a notch into Up/Down.
- It ignores clicks.

`runner.rs` enables capture (`MOUSE_CAPTURE`, `sync_mouse`) while `App::mouse_capture()` is true, that is unless `tui.mouse` is `off`. It routes `TermEvent::Mouse` to `on_mouse`. The runner draws a frame before it waits for each event, so the last frame is what the user sees when a click arrives.

Drawing is `view::draw(&mut Frame, &App)`, which dispatches to one module per panel under `view/`. Views get `&App`, so a view that reports something about the frame stores it with interior mutability. The OSC 8 pass does the same: `App::screen_links` is a `Mutex<Vec<ScreenLink>>` that `view/messages.rs` fills and the runner reads after the frame. Behavior lives in the key handlers:

- `explorer_key` and `open_row` (`explorer.rs`)
- `picker_key`, `palette_key`, `permission_key` and `question_key` (`app.rs`)
- `sidebar_key` (`sidebar.rs`), `switcher_key` (`switcher.rs`) and `workflows_key` (`workflows.rs`)
- `tree_key` (`branches.rs`), `messages_key` (`messaging.rs`) and `team_key` (`team.rs`)
- the Settings keys

`App::now()` reads `App::clock`, which snapshot tests fix (with `clock_step` for later reads).

## Decisions

### One hit map, filled while drawing

- `mouse.rs` defines a `Hits` list of `(Rect, Target)` and a `Mouse` struct holding the `Mutex<Vec<Hit>>` and the armed click. `App` gains one field, `mouse: crate::mouse::Mouse`, initialized by `Default`, which keeps `app.rs` edits small for the sibling branches.
- `view::draw` clears the hits at the start of each frame. Each view pushes targets for what it draws, in draw order: `app.mouse.push(rect, target)`. The view that computes a row's position, its scroll offset and its row windowing registers that row's rect, so no hit-testing math is repeated in `mouse.rs`.
- `on_mouse` takes the last hit that contains the pointer, which is the topmost one because later draws paint over earlier ones.
- Every panel drawn over the main screen is modal (picker, palette, theme switcher, prompt, confirmation, help, side chat, rename, login, task log, file diff, findings, `/tree`). Before it draws one, `view::draw` calls `app.mouse.modal(area)`, so no panel can forget the call. It drops every hit registered so far and adds a `Target::Inside` covering the screen, under the panel's own targets. A click outside the panel, or inside it on no row, hits `Inside` and does nothing; like any press, it also disarms an armed row. Full-screen panels (Settings, explorer, Workflow monitor, `/messages`, `/team`, collaboration) are drawn alone, so only their own targets and the footer exist.

### Targets reuse the key handlers

```rust
enum Target {
    Row { list: List, index: usize, act: Act }, // Act::Second (rows) or Act::First (controls)
    Key(KeyEvent),                              // a chip, a tab, a collapsed row: press this key
    Pane(Pane),                                 // Transcript, Composer, Sidebar, ExplorerTree, ExplorerPreview
    Inside,                                     // inside a modal, on no target
}
```

- `Row` selects through one `App::select_row(list, index) -> bool`. It is a single `match` that sets the open panel's own selection field:
  - `Picker.selected` (a position in `filtered`) and `Palette.selected`
  - the switcher, the Workflow monitor, the branch tree, messages and team selections
  - `Settings.config_row` and `Sidebar.focus`
  - `PermStep::Choose(i)` and `QuestionForm.cursor`

  It returns `false` when that list is not the one open, when the panel waits for typed input (a Session picker's pending rename or delete, a question's own answer being typed), or when the index is past its end. The click is then ignored, which fails closed on a stale frame. Activation presses Enter through `on_key`, so the existing handler does the work: a model switch, a Session resume, `open_row`, a permission reply, a theme kept.
- `Key` targets are registered with the key that the action is bound to at draw time (`Keymap::label` or the binding table that the chips already use). Settings tabs use the fixed `1`–`4`. An action bound to `"none"` registers no target. A click on a key target can therefore never do what that key could not.
- Sidebar section headers are `Row { act: First }`: focus the sidebar on the header, then press Enter (`sidebar_select`). This is the keyboard sequence `<leader>e`, move, Enter. Other sidebar rows are `Act::Second`. `Pane(Transcript | Composer)` calls `leave_sidebar()` when the sidebar has focus, as Esc does there. The click never sends Esc to the composer, where Esc interrupts a Turn.
- Collapsed transcript rows come from `view/messages.rs`. The hidden-lines row of a collapsed card registers `Key(<tool_details key>)`, and the `thinking` row registers `Key(<reasoning_toggle key>)`. Both keys toggle for the whole Session today, and the click does the same thing as the key.
- Views that draw a scrolled paragraph register its rows with one helper, `view::push_lines(app, body, lines, skip, target)`, which drops the rows scrolled away or cut off below. The sidebar's sections record the line each focusable row is drawn on as they push it, so its targets follow wrapped notes and folded sections without recomputing them. `view::draw` registers the transcript, composer and sidebar panes before drawing them, so the rows drawn on a pane are above it.
- The Workflow monitor registers no run or agent rows while a stop or revision confirmation is pending, and `select_row` refuses them then and while the log or the library is open: the confirmation takes any key, and Enter would answer it.
- The hook views draw no rows to pick: `/hooks` shows one definition at a time and `/hooks history` is a log where Enter closes. They take no clicks, and the wheel still moves them.

### Arming and double clicks

- `Mouse` keeps the last press: its target, `App::now()` at the press, and an identity of the context. The identity is the open overlay's discriminant plus, for a prompt, the pending request's ID.
- A `Row` press also records the item `select_row` selected, by a key that stays with it when the list reloads (`App::selected_item`: the picker kind and `Choice.key`, the palette entry, the explorer path, the theme, the run or agent ID, the message, member or `/tree` node ID, the sidebar row's run, server or file). A `Row` press activates only when the previous press hit the same target in the same context and selected the same item. Otherwise it selects and arms the row, so a live reload (Tasks, Children, admissions, sidebar, Workflow runs, team, `/tree`) or a picker replaced by another kind never confirms an item that was not shown selected. A double click is two presses on the same target, so it selects and then activates. No separate double-click path exists for rows.
- A control press (`Act::First`, `Key`) is absorbed when it lands on the same target or at the same cell, in the same context, within `DOUBLE_CLICK` (500 ms) of the previous press. A double click on a folded directory therefore unfolds it once, instead of unfolding and folding again.
- A press within `DOUBLE_CLICK` of the previous press but in another context is the tail of a multi-click whose earlier press opened that panel or raised that request (an explorer `ask` file, a palette or picker double click). It is absorbed: it neither selects nor arms, and it keeps the burst going, so every press of the burst is absorbed until a pause of `DOUBLE_CLICK`. An activating row press is kept with no item for the same reason, so it never arms the next press.
- A click whose pane is the transcript or the composer takes focus back from the sidebar before its target acts, so a collapsed row drawn over the transcript expands instead of reaching the sidebar's keys.
- Lists keep their scroll window: `Mouse::window(list, selected_lines, room, len)` returns the first line shown and moves it only when the selection would leave it, as ratatui's `ListState` offset does. The picker, palette, theme switcher, explorer tree, `/tree`, `/messages` and the Workflow runs and agents use it instead of deriving the window from the selection, so a click on a row above the selection does not scroll the row from under the pointer before the second click.
- Time comes from `App::now()`. Tests set `App::clock` and step it, so no test reads the wall clock.
- `App::on_key` calls `self.mouse.disarm()` on its first line, so any key press, including one that moves the selection away and back, forgets the armed row. A change of overlay or of the pending request changes the context identity, which disarms the row as well.

### Prompts never answer on one click

- Permission options and question options are `Row { act: Second }`. One click only moves `PermStep::Choose(i)` or the question cursor. The second click presses Enter, which is the reply the keyboard would send. The pending request's ID is part of the context identity, so a click armed for one request never confirms the next one.
- Only `PermStep::Choose` registers targets, and `select_row` refuses the other steps as well. Option rows of the wrapped prompt text are placed with ratatui's `Paragraph::line_count` (the `unstable-rendered-line-info` feature), so a wrapped warning or diff above them cannot shift a target off its row. `ConfirmAlways`, which saves approval patterns, the `Feedback` text, the blocked-call approval (`auto_override`) and the confirmation overlays (`ConfirmStopTasks`, `ConfirmBypass`, `ConfirmExport`) register only `Inside`. A click therefore can't save an always-allow rule, approve a classifier-blocked call, stop tasks, enter bypass mode or overwrite a file.
- Wheel handling is unchanged: prompts and confirmations still ignore it.

### Pointer-aware wheel

- With no overlay, the wheel looks up the topmost `Pane` under the pointer:
  - over `Sidebar`, it does nothing;
  - anywhere else on the main screen, it scrolls the transcript by `WHEEL_LINES`, as before (this includes no hit at all, so the PR #25 tests keep passing).
- In the explorer, a notch over `ExplorerPreview` scrolls the preview by `WHEEL_LINES`, and a notch over the tree moves the selection (Up/Down), whichever pane has focus.
- In other panels, the wheel keeps PR #25's Up/Down behavior (`wheel_moves()`).

### Explorer preview focus

- Today `preview_focused` means that the preview replaces the tree, and it is only set below `SIDE_BY_SIDE` (100) columns. It becomes "keys go to the preview" at every width. `view/explorer.rs` replaces the tree with the preview only when the width is below `SIDE_BY_SIDE`. The focused pane's header title is drawn in `accent`.
- A new `explorer` action, `explorer_focus` (default `tab`), toggles focus when a preview is open, so the preview focus a click gives is also reachable from the keyboard. Esc (`explorer_close`) in the preview already returns to the tree. `preview_key` keeps its handlers and adds `explorer_focus`.
- Tree rows are `Row { list: Explorer, index, act: First }`: select, then Enter (`open_row`). Folding, previews, `ask` files raising a permission request, and the `deny` toast all stay in `open_row`. The fold marker lies inside the row's rect, so it needs no separate target.
- The header's `rendered`/`raw` chip is `Key(m)`, from the bound `explorer_markdown` key. The footer chips, which `chips()` already builds from the binding table, register `Key` with their own key.

### Gating

- `on_mouse` returns at once unless `mouse_capture()` is true. With `tui.mouse: off` the runner also turns reporting off, so this check is a second guard.
- Screen-reader mode handles no clicks: `on_mouse` ignores presses while `App::screen_reader` is set, because its rows are drawn as sentences and the hit map would not match what a screen reader reads. The wheel keeps working there, as it did before. The runner only draws in fullscreen today, so inline rendering, when it exists, will not enable capture.
- Only `MouseEventKind::Down(MouseButton::Left)` with no modifiers counts as a click.
- No `cfg(unix)` or `cfg(windows)` production code is added. crossterm reports mouse events on both platforms. One screen test (`/tasks` and `/agent` rows) runs `sleep` and a spawned child, like `screens_tasks`, so it and its scripted reply are `cfg(unix)`.

## Testing

- `mouse::tests` (unit, against `App::new`):
  - arming and disarming on key press and on overlay change;
  - double click within 500 ms and beyond, with a stepped `App::clock`, and at the same cell on another control;
  - a picker reloaded or replaced between two clicks only re-arms;
  - list windows stay until the selection leaves them;
  - `Act::First` absorbing the second press;
  - stale indices ignored;
  - clicks with `tui.mouse: off`, in inline mode and with modifiers ignored;
  - a confirmation overlay ignoring clicks.
- Screen tests (`screens_mouse.rs` and `screens_explorer.rs`) draw a production App through `view::draw`, against the scripted local server of `screens::Server` where data is needed. They find a row's coordinates with `screens::row_of` and the column of its text, send `MouseEvent`s to `App::on_mouse`, draw again and assert the state change and the next frame.
- `runner::mouse_tests` checks that a terminal click reaches `on_mouse` through `terminal_event`.

## Risks / Trade-offs

- [Stale targets after a state change without a redraw] → The runner draws before each event. `select_row` also checks the list kind and the bounds, and a mismatch does nothing.
- [Accidental grants] → One click never answers a prompt. The second click must hit the same option of the same request with no key between. The steps that persist rules or bypass a classifier take no clicks.
- [Conflicts with `fix-tui-mouse-scroll`] → That change is not archived and also modifies "Rendering modes and terminal integration". This change's text is a superset of it, so archive `fix-tui-mouse-scroll` first.
- [Sibling branches] → The output-style picker (`p0-styles`) reuses the generic picker, so its rows get clicks from `view/overlays.rs::picker` without further work. `app.rs` changes by one field and one line.
- [Mutex in drawing] → The lock is uncontended. Draw and input run on the same task, and the lock follows the `screen_links` pattern, including poisoned-lock recovery.
