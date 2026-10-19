## Context

`cyber sessions export` builds one in-memory export document from the store. It holds the Session info, messages, parts, goals and workflow run summaries. Today that document is rendered as JSON or Markdown, and `--sanitize` rewrites the document before rendering. HTML is a third renderer of the same document, so the sanitize behavior and the content selection come for free.

## Decisions

### Self-contained, no JavaScript

- One file with inline CSS. There is no `<script>`, and no external URL in `src`, `href` or CSS `url()`.
- The page sets `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">`. If someone opens it from an untrusted place, nothing can load and no injected script can run.
- Collapsing uses `<details>/<summary>`, and the color scheme uses `@media (prefers-color-scheme: dark)`.

### Escaping and Markdown

- Every string from the Session (prompts, responses, tool input and output, file paths, titles) is HTML-escaped before it is inserted.
- Assistant and user Markdown is rendered with `pulldown-cmark`, with raw HTML disabled (raw HTML events are dropped), so a model can't inject markup.
- Links render as text with the URL shown, not as clickable anchors. That avoids `javascript:` URLs and tracking clicks, and it matches the no-network rule.
- Code blocks are `<pre><code>`, with no syntax highlighting in this change.

### Structure

```
header: title, Location, models, started/ended, turns, tokens (in/out/cache), cost
for each message on the exported path:
  user | assistant section with timestamp and model
    text (Markdown)
    reasoning  → <details> collapsed
    tool call  → <details>: name, status, duration; input (JSON, pretty); output (truncated at 20 000
                 chars with a "truncated" note); file edits as unified diff with +/- line classes
child Sessions (with --include-children) → nested <section> after the parent call that spawned them
footer: "Exported by cyber <version> on <date>"; sanitize notice when --sanitize
```

- **Branches:** if `add-session-branches` has landed, the export follows the active path by default, and `--all-branches` adds the other branches as nested sections. Without it, the export is the linear history.

### Output

- `--output <file>` writes the export atomically. Without it, the export goes to stdout, as the other formats do.
- **TUI `/export`:** writes to a file and never overwrites one without confirmation. The default name is `<title-slug>-<short-id>.<ext>` in the Location. The slug is lowercase ASCII, with runs of other characters turned into `-`, and at most 48 characters.

## Risks / Trade-offs

- [Big Sessions make big files] → Tool output is truncated per call at 20 000 characters, and the full output stays in the JSON export.
- [Secrets in the page] → The export goes through the same credential masking as the other formats, and `--sanitize` is available. The TUI notes that the export contains the full conversation.
