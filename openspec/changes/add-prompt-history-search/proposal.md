## Why

Up/Down walk the prompt history one entry at a time. With 500 entries per project, getting back to a prompt from last week means pressing Up dozens of times. Shells and other coding agents answer this with Ctrl+R reverse search, and users reach for it by habit.

## What Changes

- `tui` new requirement "Prompt history search": Ctrl+R in the composer opens an incremental search over the project's prompt history, newest first, with duplicates listed once and smart-case substring matching. Ctrl+R/Up older, Down newer, Enter/Tab/Left/Right accept for editing (never submit), Esc/Ctrl+G/Ctrl+C restore the draft.
- `tui` "Prompt editor": points to the search.
- `tui` "Keybindings": the `composer` context gains `history_search` (default Ctrl+R), which `"none"` disables.

## Impact

`crates/cyber-tui` only (composer, key handling, composer view, binding table). History storage and its 500-entry limit are unchanged, and nothing is sent to the server.
