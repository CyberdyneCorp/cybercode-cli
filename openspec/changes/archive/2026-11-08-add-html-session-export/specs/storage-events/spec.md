## MODIFIED Requirements

### Requirement: Session export
(P0) `cyber sessions export <id> [--sanitize] [--include-children] [--all-branches] [--format json|md|html] [--output <file>]` SHALL write the Session info, messages, parts, goals and workflow run summaries. `--sanitize` SHALL replace prompt and response text, file paths, URLs, tool inputs and outputs, and metadata with `[redacted:<kind>:<n>]` markers while keeping structure, token counts and timings. The command SHALL read `GET /api/v1/sessions/{id}/export?sanitize=<bool>&include_children=<bool>[&branches=all]`, whose JSON document SHALL hold `version` (1), `exported_ms`, `sanitized`, `session`, `totals`, `branches` (`active`, or `all` with `--all-branches`), `messages` (the active path, or with `--all-branches` every branch in creation order; each with its `parent_message_id`, its parts and, for answers, its tool calls), `goals` (with their evaluations), `workflow_runs` (run summaries) and, with `include_children`, `children` (the same document for each child Session, recursively). `<kind>` SHALL be `text`, `path`, `url`, `tool` or `metadata`, `<n>` SHALL number the distinct values of a kind in order of appearance so equal values keep equal markers, and IDs (including `parent_message_id`), statuses, kinds, sources, agents, models, modes and `branches` SHALL be kept. `--format md` SHALL render the same document as Markdown and `--format html` as one self-contained page (below); the default is JSON. `--output` SHALL write the export atomically to a file instead of stdout; it SHALL refuse a symlink or a non-regular file at that path, keep an existing file's permissions, and create a new file readable by its owner only.

(P1) `--format html` SHALL produce one self-contained HTML document from the same export document:

- **No network:** no scripts and no external resources, and a Content Security Policy of `default-src 'none'; style-src 'unsafe-inline'`.
- **Escaping:** all Session content SHALL be HTML-escaped. Markdown SHALL be rendered with raw HTML shown as escaped text, never as markup. Links and images SHALL appear as text followed by their URL, not as anchors or `img` elements.
- **Content:**
  - a header with title, Location, models, times, token totals and cost,
  - each message with its role, time and model,
  - tool calls in collapsible sections with input and output (truncated at 20 000 characters per call, with a note),
  - reasoning collapsed by default,
  - the Session's goals and workflow runs,
  - child Sessions nested under the call that started them when `--include-children` is given.
- **Color scheme:** the page SHALL follow `prefers-color-scheme`.
- **Sanitizing:** `--sanitize` SHALL apply to HTML exactly as to JSON: the page SHALL show only the document's redaction markers.

#### Scenario: Sanitized export
- **WHEN** the user exports with `--sanitize`
- **THEN** no file path or prompt text from the session appears in the output, and token totals are preserved

#### Scenario: Export with children
- **WHEN** the user exports a Session whose workflow run started an agent, with `--include-children`
- **THEN** the export lists the run summary and nests the agent's Session with its Goal under `children`

#### Scenario: Sanitized export of every branch
- **WHEN** the user exports a Session with two branches with `--all-branches --sanitize`
- **THEN** the document has `branches: "all"`, lists the messages of both branches, and every `parent_message_id` is kept

#### Scenario: Self-contained HTML
- **WHEN** the user runs `cyber sessions export ses_A --format html --output run.html`
- **THEN** `run.html` contains no `<script>` element and no `http`/`https` URL in any `src`, `href` or CSS `url()`, and opening it offline shows every message and tool call of the Session

#### Scenario: Injected markup is inert
- **WHEN** a model response contains `<img src=x onerror=alert(1)>` and a Markdown link `[x](javascript:alert(1))`
- **THEN** the HTML export shows both as escaped text and contains neither an `img` element nor a `javascript:` link

#### Scenario: Export through a symlink
- **WHEN** `--output` names a symlink
- **THEN** the export fails, nothing is written, and the symlink's target is unchanged
