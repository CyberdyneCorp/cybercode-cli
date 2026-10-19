## MODIFIED Requirements

### Requirement: Session export
(P0) `cyber sessions export <id> [--sanitize] [--include-children] [--format json|md|html] [--output <file>]` SHALL write the Session info, messages, parts, goals and workflow run summaries. `--sanitize` SHALL replace prompt and response text, file paths, URLs, tool inputs and outputs, and metadata with `[redacted:<kind>:<n>]` markers while keeping structure, token counts and timings. `--output` SHALL write the export atomically to a file instead of stdout.

(P1) `--format html` SHALL produce one self-contained HTML document:

- **No network:** no scripts and no external resources, and a Content Security Policy of `default-src 'none'; style-src 'unsafe-inline'`.
- **Escaping:** all Session content SHALL be HTML-escaped. Markdown SHALL be rendered with raw HTML dropped. Links SHALL appear as text, not as anchors.
- **Content:**
  - a header with title, Location, models, times, token totals and cost,
  - each message with its role, time and model,
  - tool calls in collapsible sections with input, output (truncated at 20 000 characters per call, with a note) and unified diffs for file edits,
  - reasoning collapsed by default,
  - child Sessions nested under the call that started them when `--include-children` is given.
- **Color scheme:** the page SHALL follow `prefers-color-scheme`.
- **Sanitizing:** `--sanitize` SHALL apply to HTML exactly as to JSON.

#### Scenario: Sanitized export
- **WHEN** the user exports with `--sanitize`
- **THEN** no file path or prompt text from the session appears in the output, and token totals are preserved

#### Scenario: Self-contained HTML
- **WHEN** the user runs `cyber sessions export ses_A --format html --output run.html`
- **THEN** `run.html` contains no `<script>` element and no `http`/`https` URL in any `src`, `href` or CSS `url()`, and opening it offline shows every message and tool call of the Session

#### Scenario: Injected markup is inert
- **WHEN** a model response contains `<img src=x onerror=alert(1)>` and a Markdown link `[x](javascript:alert(1))`
- **THEN** the HTML export shows both as escaped text and contains neither an `img` element nor a `javascript:` link
