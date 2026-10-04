## MODIFIED Requirements

### Requirement: Default rules
(P0) Without configuration the system SHALL apply: `* ask`; `read`, `glob`, `grep` and `list` allow within the Location; `todo`, `skill` and `history_search` allow; `edit`, `bash` and external mutations ask; `external_directory ask` except the tool-output, jobs, temp, skill and worktree directories; `read` and `edit` `ask` for `*.env` and `*.env.*` but `allow` for `*.env.example`; `question`, `plan_enter` and `plan_exit` allow for primary agents (`mode` `primary` or `all`) and deny for subagent-only and hidden agents; `doom_loop ask`; `network ask` for domains outside `sandbox.allowed_domains`; `message.send ask` for cross-machine targets; `workflow.run ask`; `remote.attach ask`.

#### Scenario: Reading .env asks
- **WHEN** the model reads `.env.local` with default rules
- **THEN** a permission request is raised

#### Scenario: Subagent cannot ask questions
- **WHEN** an `explore` subagent calls `question`
- **THEN** the call is denied and the subagent is told to return its best answer instead

#### Scenario: Session bookkeeping needs no approval
- **WHEN** the model calls `todo` or `history_search` with default rules
- **THEN** the call runs without a permission request
