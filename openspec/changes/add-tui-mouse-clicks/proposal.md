## Why

The fullscreen TUI captures the mouse since `fix-tui-mouse-scroll` (PR #25), but it only uses the wheel, and the wheel always acts on the open panel's selection whatever lies under the pointer. In the file explorer, the panel users reach for the mouse in most, a click does nothing and the wheel moves the tree selection even over the preview. Clicking a list row, a tab or a section header is what users expect from a TUI that has taken over their mouse, and the current behavior also swallows their clicks.

## What Changes

- `tui`, new requirement "Mouse clicks" (P1): one click model for every panel.
  - A **row** of a list (pickers, palette, theme switcher, `/tasks`, Workflow monitor, `/messages`, `/team`, `/tree`, Settings Config rows, prompt options) is selected by the first click and activated, as Enter does, by a second click on the same row. A double click is a second press on the same target within 500 ms.
  - A **control** (Settings tab, key chip, the explorer's `rendered`/`raw` chip, explorer tree row, sidebar section header, collapsed transcript row) acts at once, as its key does. A double click on it toggles once.
  - A **pane** (transcript, composer, explorer tree and preview) takes focus when clicked.
  - A key press, a panel change or a new request disarms a selected row. Clicks outside a modal panel do nothing and never close it.
  - Confirmations, the Allow always confirmation step, the rejection feedback and the blocked-call approval take no clicks.
  - `tui.mouse: off`, inline rendering and screen-reader mode handle no clicks.
- `tui` "Rendering modes and terminal integration" (modified): the wheel acts on the pane under the pointer, so it does nothing over the sidebar and scrolls the transcript elsewhere on the main screen. Clicks follow "Mouse clicks". This change builds on the text that `fix-tui-mouse-scroll` gives the requirement.
- `tui` "File explorer" (modified): clicking a tree row selects it and acts as Enter, so it folds or unfolds a directory or opens a file's preview. A click in the preview focuses it, the header chip toggles `rendered`/`raw`, and the key chips act as their keys. The wheel scrolls the pane under the pointer. A new `explorer_focus` binding (default Tab) moves focus between the tree and the preview, so focusing the preview is also possible from the keyboard.
- `tui` "Permission prompt" and "Question prompt" (modified): a click only selects an option. Answering takes a second click on the same option or Enter, so a stray click can never grant a permission.
- `tui` "Message and part rendering", "Background tasks view", "Subagent threads and side chat", "Workflow monitor", "Command palette", "Session sidebar", "Settings panel", "Settings Config tab", "Theme switcher" and "Branch tree view" (modified): each panel's click behavior. Clicking the transcript or the composer returns focus from the sidebar.

## Non-goals

These are deferred to later changes:

- **Drag to select text inside the TUI and copy it.** The terminal's modifier-drag selection (Shift, or Option in iTerm2) already works with capture on. An in-TUI selection would need its own buffer-to-text mapping, wrapping, and clipboard and OSC 52 handling.
- **Drag to resize panes.** Pane sizes follow the layout rules of "Responsive layout" and the explorer's fixed split, and there is no keyboard resize to mirror.
- **Right-click context menus.** Every action already has a key and a palette entry. A menu would be a second, mouse-only way to reach the same actions.
- **Clicking in the composer to place the cursor.** A click there only returns focus. Moving the cursor needs a map from screen cells to text with wrapping and attachment chips, which is follow-up work.

## Impact

`crates/cyber-tui`:

- `mouse.rs` holds the hit map, targets, click arming, double-click detection on `App::now()`, and pointer-aware wheel routing.
- `view/mod.rs` resets the hit map each frame and isolates modal panels. The views register their targets: `view/overlays.rs` (pickers, palette, prompts), `view/explorer.rs`, `view/settings.rs`, `view/sidebar.rs`, `view/messages.rs`, `view/composer.rs`, `view/themes.rs`, `view/workflows.rs`, `view/branches.rs`, `view/team.rs` and `view/messaging.rs`.
- `explorer.rs` and `keys.rs` add the `explorer_focus` binding and preview focus in wide terminals.
- `app.rs` gets one field and one line that disarms on key presses.

No server, API, OpenAPI or SDK change. Docs: the local core guide (the mouse paragraph and the explorer section), the feature reference TUI row and the README TUI row.
