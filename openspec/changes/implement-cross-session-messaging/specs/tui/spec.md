## ADDED Requirements

### Requirement: Cross-session message surfaces
(P1) The TUI SHALL render a received cross-session message in the transcript as a block labelled with the sender's address, Session ID and time and marked untrusted; a `send_message` call with its target, outcome (and reason) and message ID; and an idle notice as one line with the target, time and status. The footer SHALL show `✉ <n> queued` and `✉ <n> held` while the Session has unpromoted or held messages. Held messages SHALL appear as cards above the composer with actions to release (`r`), drop (`d`) and view (Enter), using the inbox release and drop routes. `/messages` (also in the command palette, binding `messages_view`, unbound by default) SHALL list the Session's message history with time, direction, peer address, outcome, size and the first line of text, filterable by direction and held state, expandable to the full text, and SHALL allow releasing and dropping held messages and clearing a `non_interactive` refusal override. All of these SHALL be built from durable events and routes, so they are restored after reconnect.

#### Scenario: Review traffic
- **WHEN** the user opens `/messages` in a Session that sent one message and received two
- **THEN** three rows are shown with their directions, outcomes and timestamps

#### Scenario: Release from the card
- **WHEN** a message is held in the attached Session and the user presses `r` on its card
- **THEN** the message is released, the card disappears, the held count in the footer drops by one, and the message is promoted at the next Safe Boundary
