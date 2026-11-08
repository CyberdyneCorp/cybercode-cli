## Context

`cyber sessions export` reads one export document from the server (`GET /api/v1/sessions/{id}/export`, `add-session-export`). It holds the Session info, totals, messages with their parts and tool calls, goals, workflow run summaries and, with `include_children`, the same document for each child Session. The server applies `sanitize` before the document leaves it. JSON and Markdown render that document today; HTML is a third renderer of the same document, so the sanitize behavior and the content selection come for free.

## Decisions

### One document, rendered on the client

- The server's document is the only source; no client builds its own. Sanitizing stays on the server, so every format and every client redacts the same values with the same markers.
- The renderers (JSON, Markdown, HTML) and the atomic writer live in `cyber_client::export`, so the CLI and the TUI (`/export`) share them; both already depend on `cyber-client`. The Markdown renderer moved there from `cyber-cli`.
- `--format html` extends the global `--format` (the only output-format flag). Any other command refuses `html` with exit code 2.

### Self-contained, no JavaScript

- One file with inline CSS. There is no `<script>`, and no external URL in `src`, `href` or CSS `url()`.
- The page sets `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">`. If someone opens it from an untrusted place, nothing can load and no injected script can run.
- Collapsing uses `<details>/<summary>`, and the color scheme uses `@media (prefers-color-scheme: dark)`.

### Escaping and Markdown

- Every string from the Session (prompts, responses, tool input and output, file paths, titles) is HTML-escaped before it is inserted.
- Assistant and user Markdown is rendered with `pulldown-cmark` by our own event writer, which emits only its own tags: raw HTML events are written as escaped text, so a model can't inject markup, and a reader still sees what the model wrote.
- Links render as text with the URL shown, not as clickable anchors, and images as `[image: alt]` with the URL. That avoids `javascript:` URLs and tracking clicks, and it matches the no-network rule. No attribute ever carries Session content.
- Code blocks are `<pre><code>`, with no syntax highlighting in this change.

### Structure

```
header: title, Location, models, started/ended, turns, tokens (in/out/reasoning/cache), cost
for each message of the document (the active path, or every branch with --all-branches):
  user | assistant | system section with timestamp and model
    text (Markdown)
    reasoning  → <details> collapsed
    tool call  → <details>: name, status, attempt; input (JSON, pretty); output (truncated at 20 000
                 chars with a "truncated" note)
child Sessions (with --include-children) → nested <section> after the call whose input or output
  names the child; children no call names (for example after --sanitize) go at the end
goals and workflow runs → lists after the messages
footer: "Exported by cyber <version> on <exported_ms>"; sanitize notice when sanitized
```

- **File changes:** the document has no per-Turn diff, so the page shows a file edit as the edit tool's input (`write`, `edit`, `apply_patch`). Adding each Turn's unified diff to the server document (and to its OpenAPI schema) is a follow-up; the client-built document of the first draft fetched `.../diff?message_id=` per Turn, which this change drops with that document.
- **Branches:** the export follows the active path by default; `--all-branches` exports every branch in creation order, and the header says so.

### Output

- `--output <file>` writes the export atomically: a temporary file in the target's directory, synced and renamed over it. A symlink or a non-regular file at the path is refused, so the export never lands where the name does not show. An existing file keeps its permissions; a new one is private to its owner (`0600`), since it holds the whole conversation. Without `--output`, the export goes to stdout, as the other formats do.
- **TUI `/export`:** writes to a file and never overwrites one without confirmation. The default name is `<title-slug>-<short-id>.<ext>` in the Location. The slug is lowercase ASCII, with runs of other characters turned into `-`, and at most 48 characters.

## Risks / Trade-offs

- [Big Sessions make big files] → Tool output is truncated per call at 20 000 characters, and the full output stays in the JSON export.
- [Secrets in the page] → The export goes through the same credential masking as the other formats, and `--sanitize` is available. The TUI notes that the export contains the full conversation.
- [No per-Turn diffs yet] → File edits are visible through the edit tools' inputs; the diff section is a follow-up on the server document.
