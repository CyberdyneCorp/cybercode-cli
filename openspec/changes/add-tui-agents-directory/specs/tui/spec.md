## ADDED Requirements

### Requirement: Agent directory
(P1) `/agents` SHALL open a full-screen directory of every Session and agent the attached Session can reach by message. It SHALL also be in the command palette, with the binding `agents_view`, which is unbound by default. The directory SHALL read these sources:

- the messaging listing `GET /api/v1/messaging/sessions?from=<attached Session>`: subagents, teammates, other local Sessions, and direct-peer Sessions (`name@peer`, with `*@peer` for an offline peer);
- when project collaboration is enabled and the Session is bound to a project, the project's active hub agents and members from the collaboration participants route, except the signed-in member and the agent the attached Session is published as.

**Rows.** Rows SHALL be grouped as **This server**, **Peers** and **Project hub**. Each row SHALL show:

- the address to send to;
- a kind: `subagent`, `workflow agent`, `teammate`, `session`, `peer`, `hub agent` or `hub member`;
- a state: `idle`, `running`, `waiting_input` or `offline` (`online` or `offline` for hub agents; a hub member has no presence on the hub and shows `-`), as a glyph and a word;
- whether it can receive;
- its Location: the directory, the peer name, or `hub:<project>`.

**Filtering.** Typed characters SHALL filter the rows on address, name, kind and Location. Tab SHALL cycle a kind filter. Esc SHALL clear a non-empty filter before it closes the overlay.

**Selection and refresh.** The selection SHALL be kept by row identity across refreshes. The directory SHALL refresh on open, every 10 seconds while open (not while an editor is open), and after an action.

**Actions.** The selected row SHALL offer:

- **message:** Enter opens a compose editor. A listing row is sent through `POST /api/v1/messaging/send` from the attached Session. A hub row is sent as a direct message through the collaboration messages route. The outcome (`delivered`, `held` or `refused` with its reason) or the error SHALL be shown. An outcome SHALL close or annotate only the editor that submitted it; once that editor is closed, it SHALL be shown as a notice.
- **watch:** Ctrl+W, through `POST /api/v1/messaging/watch`.
- **open:** Ctrl+O switches the TUI to a local Session. On a peer row it shows the `cyber attach <session>@<peer>` command.
- **rename:** Ctrl+R, for local Sessions and subagents only, through the same rename used by `/rename` and the Session picker.
- **held messages:** Ctrl+G opens `/messages` filtered to held messages.

An action that does not apply to the row SHALL do nothing and say why. The directory SHALL use only the existing routes and SHALL NOT bypass inbound rules, rate limits or permissions.

**Untrusted text.** Every name, address, Location and hub text SHALL be stripped of control and bidi characters before display. A row whose address changes under that cleaning SHALL be sent to by its Session ID or not at all. Peer rows SHALL NOT be offered open-as-local or rename.

**Errors and empty states.** A source that fails SHALL show its error in its group without hiding the other sources:

- messaging disabled for the Location SHALL be shown as disabled (rows that cannot receive, or a disabled line when the listing itself is unavailable);
- a failed messaging refresh SHALL keep the last rows;
- a failed hub read SHALL clear the hub rows.

With no reachable rows, the directory SHALL say so and point to peer pairing and `/collab`. Under NO_COLOR, state SHALL be conveyed by glyph and word, and the selection by reverse video. A Session switch SHALL close and reset the directory.

#### Scenario: Local Sessions and a subagent
- **WHEN** the attached Session has one subagent named `tests`, and a Session titled `migration` runs in another directory on the same server, and the user runs `/agents`
- **THEN** **This server** lists `tests` as `subagent` and `migration` as `session` with its state and directory, and the attached Session itself is not listed

#### Scenario: Offline peer
- **WHEN** a configured peer `nas` cannot be reached and the user opens `/agents`
- **THEN** **Peers** shows `*@nas` as `peer`, `offline`, `closed`, and pressing Enter on it shows that it cannot receive instead of opening the editor

#### Scenario: Message from the directory
- **WHEN** the user types `migr`, presses Enter on `migration`, writes `schema is merged` and presses Enter
- **THEN** the message is sent from the attached Session to `migration`, the toast shows `delivered` and the message ID, and the filter still shows only `migration`

#### Scenario: Rename a local Session
- **WHEN** the user selects an untitled local Session listed by its `ses_` ID, presses Ctrl+R and enters `api-server`
- **THEN** the Session is renamed, and after the refresh its row's address is `api-server`

#### Scenario: Hub agent
- **WHEN** collaboration is enabled, the Session is bound to project `web`, and the hub lists an online agent `reviewer`
- **THEN** **Project hub** shows `reviewer` as `hub agent`, `online`, at `hub:web`; Enter sends a direct hub message to it; and Ctrl+W and Ctrl+R explain that they do not apply

#### Scenario: Crafted peer name
- **WHEN** a peer lists a Session whose name contains a bidi override and an escape sequence
- **THEN** the row shows the name without those characters, and actions on it send to the Session ID rather than to the crafted address
