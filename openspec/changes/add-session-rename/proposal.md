## Why

Users want to name a Session and pick it up later. The server already renames through `PATCH /api/v1/sessions/{id}` and the TUI Session picker had Ctrl+R, but the TUI had no `/rename`, `cyber sessions` was not implemented, user titles were stored without cleaning, and nothing stated that a user-set title survives title generation.

## What Changes

- `session-runtime` "Fork, rename, archive, delete": a user-set title is trimmed, whitespace runs become one space, control characters are dropped and it is capped at 100 characters; an empty title is `InvalidRequestError`; a generated title never replaces a user-set one, including a title request already in flight.
- `tui` "Session picker and actions": `/rename [title]` renames the current Session, or opens a prefilled dialog without an argument; it is in `/help`, the command palette (**Rename session**) and the binding table (`session_rename`, unbound by default); the header, sidebar and window title follow `session.renamed.1`.
- `cli-commands` "Resource command groups": `cyber sessions list|show|rename|delete` ship; a Session is named by ID, exact title or unique ID prefix, shared with `cyber -r` and `cyber exec --session`; an ambiguous name exits 2 and lists the matching IDs; `delete` needs `--yes`. The other `sessions` subcommands stay unavailable.

## Impact

`crates/cyber-server` (title cleaning), `crates/cyber-tui` (`/rename`, dialog, palette, event), `crates/cyber-cli` (`cyber sessions`, the shared resolver). No API shape change.
