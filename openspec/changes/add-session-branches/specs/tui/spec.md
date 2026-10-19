## ADDED Requirements

### Requirement: Branch tree view
(P1) The system SHALL provide `/tree`, listing the current Session's user messages as a collapsible tree. The tree SHALL highlight the active path, show the number of branches at each branch point, and show labels as tags. Keys in the tree:

- **Enter** SHALL check out the selected branch.
- **`c`** SHALL toggle restoring code with the checkout, with the same diff preview as `/rewind`.
- **`l`** SHALL label the selected message.

`/label <name>` SHALL label the active tip, and `/label --clear <name>` SHALL remove a label. When the Session has more than one tip, the header SHALL show `branch <label or short id> (<n> of <total>)`. `/tree` and `/label` SHALL appear in `/help` and in the command palette (**Session tree**, **Label branch**).

#### Scenario: Switch branches from the tree
- **WHEN** the Session has two branches after a rewind and the user selects the inactive one in `/tree` and presses Enter
- **THEN** the transcript shows that branch's messages and the header shows `branch <id> (1 of 2)`

#### Scenario: Busy Session
- **WHEN** the user presses Enter in `/tree` while a Turn is running
- **THEN** the TUI reports that the Session must be idle and the active branch does not change
