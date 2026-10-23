## MODIFIED Requirements

### Requirement: Session export
(P0) `cyber sessions export <id> [--sanitize] [--include-children] [--all-branches] [--format json|md]` SHALL write the Session info, messages, parts, goals and workflow run summaries. `--sanitize` SHALL replace prompt and response text, file paths, URLs, tool inputs and outputs, and metadata with `[redacted:<kind>:<n>]` markers while keeping structure, token counts and timings. The command SHALL read `GET /api/v1/sessions/{id}/export?sanitize=<bool>&include_children=<bool>[&branches=all]`, whose JSON document SHALL hold `version` (1), `exported_ms`, `sanitized`, `session`, `totals`, `branches` (`active`, or `all` with `--all-branches`), `messages` (the active path, or with `--all-branches` every branch in creation order; each with its `parent_message_id`, its parts and, for answers, its tool calls), `goals` (with their evaluations), `workflow_runs` (run summaries) and, with `include_children`, `children` (the same document for each child Session, recursively). `<kind>` SHALL be `text`, `path`, `url`, `tool` or `metadata`, `<n>` SHALL number the distinct values of a kind in order of appearance so equal values keep equal markers, and IDs (including `parent_message_id`), statuses, kinds, sources, agents, models, modes and `branches` SHALL be kept. `--format md` SHALL render the same document as Markdown; the default is JSON.

#### Scenario: Sanitized export
- **WHEN** the user exports with `--sanitize`
- **THEN** no file path or prompt text from the session appears in the output, and token totals are preserved

#### Scenario: Export with children
- **WHEN** the user exports a Session whose workflow run started an agent, with `--include-children`
- **THEN** the export lists the run summary and nests the agent's Session with its Goal under `children`

#### Scenario: Sanitized export of every branch
- **WHEN** the user exports a Session with two branches with `--all-branches --sanitize`
- **THEN** the document has `branches: "all"`, lists the messages of both branches, and every `parent_message_id` is kept
