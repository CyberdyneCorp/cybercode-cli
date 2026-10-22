## MODIFIED Requirements

### Requirement: Session export
(P0) `cyber sessions export <id> [--sanitize] [--include-children] [--format json|md]` SHALL write the Session info, messages, parts, goals and workflow run summaries. `--sanitize` SHALL replace prompt and response text, file paths, URLs, tool inputs and outputs, and metadata with `[redacted:<kind>:<n>]` markers while keeping structure, token counts and timings. The command SHALL read `GET /api/v1/sessions/{id}/export?sanitize=<bool>&include_children=<bool>`, whose JSON document SHALL hold `version` (1), `exported_ms`, `sanitized`, `session`, `totals`, `messages` (each with its parts and, for answers, its tool calls), `goals` (with their evaluations), `workflow_runs` (run summaries) and, with `include_children`, `children` (the same document for each child Session, recursively). `<kind>` SHALL be `text`, `path`, `url`, `tool` or `metadata`, `<n>` SHALL number the distinct values of a kind in order of appearance so equal values keep equal markers, and IDs, statuses, kinds, sources, agents, models and modes SHALL be kept. `--format md` SHALL render the same document as Markdown; the default is JSON.

#### Scenario: Sanitized export
- **WHEN** the user exports with `--sanitize`
- **THEN** no file path or prompt text from the session appears in the output, and token totals are preserved

#### Scenario: Export with children
- **WHEN** the user exports a Session whose workflow run started an agent, with `--include-children`
- **THEN** the export lists the run summary and nests the agent's Session with its Goal under `children`
