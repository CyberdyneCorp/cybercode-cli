## Why

The fullscreen TUI entered the alternate screen but never enabled mouse reporting, so the wheel reached the terminal: it scrolled the terminal's own scrollback (the shell history behind the UI) instead of the transcript. The `tui` spec already requires mouse support in fullscreen mode.

## What Changes

- `tui` "Rendering modes and terminal integration": fullscreen mode captures the mouse. A wheel notch scrolls the transcript by 3 lines, or moves Up/Down in a panel that scrolls or lists (pickers, palette, settings, explorer, workflows, task logs, findings, diffs, hook lists, messages, team, tree). Prompts and confirmations ignore the wheel. `tui.mouse: off` leaves the mouse to the terminal for native text selection; with capture on, terminals keep native selection on a modifier drag (Shift in most terminals, Option in iTerm2).
- Clicks are not handled yet; click targets are a follow-up change.

## Impact

`crates/cyber-tui` (`mouse.rs`, the runner's terminal setup and event routing, the `mouse` preference). No server, API or SDK change.
