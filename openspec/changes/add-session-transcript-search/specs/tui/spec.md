## ADDED Requirements

### Requirement: Session picker transcript matches
(P1) The Session picker (`/resume`) SHALL keep its title matches. When the filter text has at least 3 characters and has not changed for 250 ms, the picker SHALL also call `GET /api/v1/sessions/search` with scope `project` and show an **In transcripts** section with up to 10 Sessions. Each SHALL show its title, its relative update time and its best snippet with the highlighted words.
- Tab inside the picker SHALL switch that section between scope `project` and `all`.
- Choosing a transcript match SHALL resume that Session and scroll the transcript to the hit's message. That message SHALL stay highlighted until the next keypress.
- A hit off the active path SHALL open the branch tree view at that message instead.
- While the index backfill runs, the section header SHALL show `indexing <N>%`.
- A failed or disabled search SHALL hide the section and leave title matching working.

#### Scenario: Jump to the matching message
- **WHEN** the user types `flaky login` in `/resume` and chooses a Session from **In transcripts** whose hit is message 37
- **THEN** the TUI resumes that Session with message 37 scrolled into view and highlighted

#### Scenario: Short filter stays local
- **WHEN** the user types `fl` in `/resume`
- **THEN** only title matches are shown and no search request is sent

#### Scenario: Match on an inactive branch
- **WHEN** the chosen hit has `active_path: false`
- **THEN** the TUI opens the branch tree view at that message and does not change the active branch by itself
