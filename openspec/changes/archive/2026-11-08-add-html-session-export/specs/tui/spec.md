## ADDED Requirements

### Requirement: Export command
(P1) The system SHALL provide `/export [html|md|json] [path]` to export the current Session with the same renderers as `cyber sessions export`. The default format SHALL be `html`. The default path SHALL be `<title-slug>-<short-id>.<ext>` in the Location, where the slug is lowercase ASCII with other characters collapsed to `-`, at most 48 characters. The TUI SHALL ask before overwriting an existing file, SHALL print the written path, and SHALL note that the export contains the full conversation. `/export` SHALL be listed in `/help` and in the command palette (**Export session**).

#### Scenario: Default HTML export
- **WHEN** the user runs `/export` in a Session titled `Auth refactor` with ID `ses_01M58P862K…`
- **THEN** `auth-refactor-01m58p86.html` is written in the Location and its path is shown

#### Scenario: Existing file
- **WHEN** the target file already exists
- **THEN** the TUI asks before overwriting it and writes nothing if the user declines
