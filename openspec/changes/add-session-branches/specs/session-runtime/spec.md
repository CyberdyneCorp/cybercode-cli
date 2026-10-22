## ADDED Requirements

### Requirement: Session branches
(P1) A Session's messages SHALL form a tree, and the Session SHALL have one active path from its active tip to the root. Only messages on the active path SHALL be sent to the model.

- **Rewind keeps the branch:** a committed conversation rewind (target `conversation` or `both`) to a user message SHALL move the active tip to that message's parent and keep the message and its descendants as an inactive branch. The next admitted prompt SHALL become a sibling of the rewound message.
- **Checkout:** checking out a message SHALL make active the path to the most recently created tip under it, or to the message itself with `exact`. Checkout SHALL require the Session to be idle (no Drain, no promotable input, no compaction in progress and no running background Job or delegation, whose result would be handed back into the newly active path), checked again when the checkout is recorded, and fail with `SessionBusyError` otherwise; a refused checkout SHALL put back any files it restored. When requested, it SHALL restore the working tree to the snapshot after that tip's last step, through the three-phase revert. Checkout SHALL record `session.branch.checked_out.1`.
- **Labels:** labels matching `[A-Za-z0-9._-]{1,48}` SHALL be unique per Session. They SHALL be set or cleared with `session.branch.labeled.1`, and SHALL be usable wherever a message ID is accepted for checkout.
- **Compaction:** a Context Epoch summary SHALL be reused only when the messages it replaced are an ancestor prefix of the active path. Otherwise the context SHALL be assembled from the path's stored messages. While a compaction is in progress, a checkout or the commit of a conversation rewind SHALL fail with `SessionBusyError`, and a summary whose retained tail is not on the active path when it completes SHALL be discarded. A compaction that ends without recording an outcome (an interrupt, a disconnected client) SHALL NOT leave the Session refusing them.
- **Existing Sessions:** replaying a Session whose events contain earlier conversation rewinds SHALL produce the corresponding branches, with the same active path as before. When such a log admits a rewound message ID again, both attempts SHALL be kept: the earlier one, with its descendants, under the derived ID `<id>~<n>`.

#### Scenario: Rewind keeps the old attempt
- **WHEN** a Session has user messages 1, 2, 3, the user rewinds the conversation to message 2, and then sends message 2b
- **THEN** the active path is 1 → 2b, and messages 2 and 3 remain as an inactive branch visible in the tree

#### Scenario: Return to an earlier attempt
- **WHEN** the user checks out message 3 of that inactive branch
- **THEN** the next Turn's context contains messages 1, 2 and 3 and not 2b, and `session.branch.checked_out.1` is recorded

#### Scenario: Checkout while running
- **WHEN** a client checks out a branch while the Session's Drain is running
- **THEN** the request fails with `SessionBusyError` and the active path is unchanged

#### Scenario: Checkout while a background Job runs
- **WHEN** a Turn on branch A started a background Job and a client checks out branch B while the Job runs
- **THEN** the request fails with `SessionBusyError`, and the Job's output never reaches a model request on B

#### Scenario: Checkout during a compaction
- **WHEN** a client checks out sibling branch B while a manual compaction of branch A is writing its summary
- **THEN** the request fails with `SessionBusyError`, and no model request on B contains A's summary, also after a restart

#### Scenario: Summary of a sibling is not reused
- **WHEN** branch A was compacted and the user checks out sibling branch B, which shares only message 1 with A
- **THEN** the context for B is assembled from B's stored messages and contains no part of A's summary
