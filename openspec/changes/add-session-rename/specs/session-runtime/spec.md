## MODIFIED Requirements

### Requirement: Fork, rename, archive, delete
(P0) The system SHALL fork a Session from an optional message (copying history before it with fresh IDs into a new Session titled `<title> (fork #N)`), rename it, archive it (hidden from default lists but resumable), and delete it (cascading to child Sessions, inbox rows, snapshots metadata, and published `session.deleted.1` events). A user-set title SHALL be trimmed, with runs of whitespace (newlines included) joined into one space, control characters removed and at most 100 characters kept; a title that is empty after cleaning SHALL fail with `InvalidRequestError` and change nothing. Once a user sets a title, a generated title SHALL never replace it, including the result of a title request already in flight.

#### Scenario: Fork from a message
- **WHEN** the user forks at message 12 of 20
- **THEN** a new Session contains copies of messages 1–11 and the original is unchanged

#### Scenario: Rename is cleaned
- **WHEN** a client renames a Session to `"  Auth\n refactor\u001b]0;x\u0007 "`
- **THEN** its title is `Auth refactor]0;x`

#### Scenario: Empty rename refused
- **WHEN** a client renames a Session to `" \t "`
- **THEN** the request fails with `InvalidRequestError` and the title is unchanged

#### Scenario: User title survives title generation
- **WHEN** the user renames a Session to `Mine` while the title request for its first prompt is still running
- **THEN** the generated title is discarded and the title stays `Mine`
