## Why

The ROADMAP's next P1 item after OpenTelemetry export is the TUI agent directory. A user in the TUI cannot see which Sessions the attached Session can message. The only ways to find out are `cyber messages list --reachable` in another terminal and the model's `list_sessions` tool. Nothing in `crates/cyber-tui` reads `GET /api/v1/messaging/sessions` today. People who run several Sessions, subagents, teammates and paired peers want one place to:

- find a Session,
- see its exact address and state,
- give it a usable name,
- message or watch it without writing a prompt that asks the model to do it.

The project hub's People list (`/collab`) is a second roster of agents that can be messaged. It is shown in another overlay with different actions.

## What Changes

- **`/agents` overlay** (`tui`, added "Agent directory"): a full-screen directory of every Session and agent the attached Session can reach right now. It is also in the command palette, with the binding `agents_view`, unbound by default.
- **Sources** (a small source list that later transports, such as the Relay, can extend):
  1. **Messaging listing:** `GET /api/v1/messaging/sessions?from=<attached Session>`. It holds subagents, teammates, other local Sessions and direct-peer entries (`name@peer`, with `*@peer` for an offline peer).
  2. **Project hub People:** `GET /api/v1/collaboration/status`, then `/collaboration/participants`. This source is read only when collaboration is enabled and the Session is bound to a project.
- **Rows:** each row shows:
  - the address to send to,
  - the kind: `subagent`, `workflow agent`, `teammate`, `session` (local), `peer`, `hub agent` or `hub member`,
  - the state: `idle`, `running`, `waiting_input`, `offline`, and for hub rows `online` or `offline`,
  - whether it can receive,
  - the Location: the local directory, the peer name, or `hub:<project>`.
- **Actions on the selected row:**
  - **Message:** an inline compose editor. It sends through the existing `POST /messaging/send` for listing rows, and the existing direct-message `POST /collaboration/messages` for hub rows.
  - **Watch:** `POST /messaging/watch`.
  - **Open:** switches the TUI to a local Session. For a peer row, it shows the `cyber attach` command.
  - **Rename:** for a local Session or a subagent, reusing the existing rename machinery (`PATCH /sessions/{id}`).
  - **Held messages:** jumps to `/messages`, filtered to held messages.
- **Interaction:** filter-as-you-type on address, name, kind and Location; Tab cycles a kind filter; periodic refresh while open; mouse selection and wheel.
- **Listing provenance** (`cross-session-messaging`, added "Reachable session provenance"). The listing gains the facts the directory needs to name kinds correctly:
  - A member of the caller's team is listed with kind `teammate`, addressed by its member name. Today it is listed as `local`, which contradicts "Reachable session listing".
  - A workflow agent's child Session carries `workflowRunId`.
  - Both changes are additive. No transport, route or permission changes.

## Non-goals

- **Relay machines** (`add-account-relay-messaging`, P3). The source list is an enum and a loader per source, so a Relay source can be added without touching rows or actions.
- **The Agent view** (`tui`, P2; `cyber agents` or Ctrl+A), which is a server-wide status dashboard with task dispatch. The directory is the reach of one Session. The two may share row rendering later.
- **New transports, routes or permission rules.** Sends and watches go through the existing routes and keep their existing rules.
- **Bulk actions** (message several rows at once) and sending to `all`.
- **Attaching to peers from inside the TUI.** The directory shows the `cyber attach <session>@<peer>` command; it does not attach.

## Impact

- **Specs:**
  - `tui`: "Agent directory" is added.
  - `cross-session-messaging`: "Reachable session provenance" is added.
  - No requirement is modified.
- **Code:**
  - `crates/cyber-tui`:
    - `agents.rs` (new): the source list, row model, state, requests and keys.
    - `view/agents.rs` (new).
    - `screens_agents.rs` (new) and its snapshots.
    - `app.rs`: an `Overlay::Agents` variant, `Action::Directory`, the slash, binding and session-switch reset.
    - `perform.rs` and `runner.rs`: `Msg::Directory`, apply and poll.
    - `palette.rs`, `keys.rs`, `mouse.rs` (`List::Directory`), `view/mod.rs`, and the help rows in `view/overlays.rs`.
  - `crates/cyber-server`:
    - `runtime/messaging/address.rs` (`reachable`): teammate kind and the member-name address; `workflow_run_id`.
    - `runtime/messaging/mod.rs`: an optional `workflowRunId` on `ReachableSession`.
    - The OpenAPI document.
  - The TypeScript SDK is regenerated.
- **Docs:**
  - `docs/local-core-guide.md`: the TUI messaging paragraph and the clickable-panels list.
  - `docs/features.md`: the Cross-session messaging, Direct peers and Project collaboration rows.
  - `docs/collaboration.md`: a pointer to `/agents`.
  - `ROADMAP.md`: the status line.
