## Why

In Pi, one session holds every attempt. You go back to an earlier message, take a different route, and can still return to the first attempt (`/tree`, labels). Cyber Code offers two partial alternatives:

- `/rewind` with target `conversation` or `both` drops the later messages from the model's view.
- `fork` copies the history into a new Session.

The first loses the abandoned attempt from every client view. The second scatters one investigation across several Sessions. Users exploring alternatives, such as two fixes for one bug, need to compare and switch between them.

The data is already there. Sessions are event-sourced, and a conversation rewind only cuts the projection (`truncate_from`). The abandoned messages stay in the event log. Branching is mostly a projection and client change.

## What Changes

- `session-runtime`, new requirement "Session branches" (P1):
  - A Session's messages form a tree, and only the active path is sent to the model.
  - A conversation rewind keeps the later messages as an inactive branch instead of hiding them for good. The next prompt starts a new branch from the rewind point.
  - `checkout` makes another branch active: the Session must be idle, the context is rebuilt from that path, and code can optionally be restored to that branch's snapshot.
  - Branch tips can carry labels.
  - Compaction summaries are reused only on the path they summarize.
- `snapshots-checkpoints`, modified "Rewind targets": `conversation` and `both` move the active branch and keep later messages as an inactive branch.
- `tui`, new requirement "Branch tree view" (P1): `/tree` shows the user-message tree with the active path, branch points and labels, and switches branches. `/label <name>` labels the active tip.
- `server-api`, new requirement "Session branch routes" (P1):
  - `GET .../tree`, `POST .../checkout`, `PUT|DELETE .../labels/:name`.
  - `GET .../messages` returns the active path unless `?branches=all` is passed.
- `cli-commands`, new requirement "Session branch commands" (P1): `cyber sessions tree <session>`, `cyber sessions checkout <session> <message|label> [--code]`, `cyber sessions label <session> <name> [--at <message>] [--delete]`.

## Non-goals

- Deleting or pruning branches. Abandoned branches stay until the Session is deleted, as their events do today.
- Merging two branches, or summarizing one branch into another (Pi's branch summaries).
- Branching inside workflow runs, subagent Sessions' parents, or team boards. Child Sessions can still be branched on their own.
- Changing `fork`, which still creates a new Session.

## Impact

- `crates/cyber-server`: new events, the Session model projection, context assembly from the active path, branch-aware compaction reuse, new routes. No schema migration is needed, because branches are rebuilt from events (see design).
- `crates/cyber-tui` (`/tree`, `/label`), `crates/cyber-cli` (`sessions tree|checkout|label`), `sdk/typescript` (the regenerated client), and the docs.
