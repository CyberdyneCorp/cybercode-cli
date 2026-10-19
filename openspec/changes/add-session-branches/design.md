## Context

A Session's state is a projection of its event stream (`session.*` events, consecutive `seq`). The model view is the ordered list of entries the projection keeps. A conversation rewind is a three-phase revert (`session.reverted.1` with phases stage, clear and commit). When it commits with target `conversation` or `both`, `truncate_from(message_id)` removes the chosen user message and everything after it from the projection. The events stay in the log, but no client can see or reach those messages again.

## Goals / Non-Goals

Goals:
- A rewind never loses an attempt.
- Any earlier attempt can be made active again.
- The TUI, CLI and API can show the tree.
- Older clients that read `GET .../messages` keep seeing one linear conversation.

Non-goals: listed in the proposal.

## Decisions

### Tree from events, no schema change

Every projected message gets a `parent_message_id`:
- The first message's parent is null.
- A message admitted after a committed conversation rewind to message M gets M's parent, which makes it M's sibling.
- Any other message's parent is the previous message on the active path.

The projection keeps every message in a `BTreeMap<MessageId, Node>` together with an `active_tip`. The model view is the path from `active_tip` up to the root.

Because the projection is rebuilt from events, existing Sessions gain a tree on their next replay: earlier rewinds become branches automatically. That needs no migration and is an intended benefit.

### Events

- `session.reverted.1` with `phase: commit` no longer truncates. The projection moves `active_tip` to the chosen message's parent (`null` when the rewind goes to the first message), and the cut messages stay as an inactive branch. The event shape does not change.
- `session.branch.checked_out.1 { tip_message_id, code_restored: bool, snapshot_id? }` makes the path ending at `tip_message_id` active.
- `session.branch.labeled.1 { name, message_id | null }` sets or clears a label. Names match `[A-Za-z0-9._-]{1,48}` and are unique per Session. A label points at a message; when that message stops being a tip, the label stays on it.

### Checkout

- **Idle only.** The Session must be idle: no Drain running and an empty promotable inbox. Otherwise the result is `SessionBusyError`, exactly as revert does today. Held inbox items stay held.
- **Choosing the tip.** The target can be any message. If it has descendants, checkout goes to the most recently created tip under it, so the active path always ends at a leaf. `--exact` selects the message itself; the next prompt then branches there.
- **Code.** With `code: true`, checkout restores the working tree to the snapshot taken after the target tip's last step. This uses the existing three-phase revert machinery, so user edits are preserved and the diff is previewed. Without it, files are left alone, and the TUI warns when the branch's last snapshot differs from the working tree.
- **Tool recovery.** Tool-recovery records keep referencing durable operation IDs, as they do today. Unknown-outcome calls on inactive branches stay reported in recovery views.

### Context assembly and compaction

- The model context is built from the active path only.
- A Context Epoch summary records the message range it replaced. It is reused only when that range is an ancestor prefix of the active path. Otherwise the context is assembled from the path's stored messages, and normal auto-compaction applies if it overflows.
- Switching back to a branch therefore never shows the model a summary of a sibling branch.

### Listing and compatibility

- `GET .../messages` returns the active path in order. Old clients see a linear conversation, and after a rewind that conversation looks exactly as it does today.
- `?branches=all` returns every message with `parent_message_id`.
- `GET .../tree` returns compact nodes for user messages only: `{ id, parent_id, preview, created_at, labels, active, tip, descendants }`.
- Export (`storage-events` → Session export) writes the active path by default, and every branch with `--all-branches`.

### Clients

- **TUI `/tree`:** a collapsible tree of user messages. The active path is highlighted, branch points show a count, and labels appear as tags.
  - Enter checks out a branch. A `c` toggle adds the code restore, and its diff preview is the same as `/rewind`'s.
  - `l` labels a branch.
  - `/label <name>` labels the active tip.
- **CLI:** `cyber sessions tree` prints an ASCII tree, or nodes as JSON with `--format json`. `checkout` takes a message ID, a unique ID prefix or a label.

## Risks / Trade-offs

- [The projection grows with abandoned branches] → Message bodies already exist as events. Branch nodes only add IDs and a parent pointer. Very large Sessions are bounded by existing retention.
- [A replay now keeps messages that older builds dropped] → This is intended. The tests pin that a Session with a pre-change rewind replays into a two-branch tree with the same active path.
- [A user forgets which branch is active] → The TUI header shows `branch <label or short id> (2 of 3)` whenever the Session has more than one tip.
