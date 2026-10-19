## Why

Pi exports a session to a self-contained HTML page (`--export`, `/export`), which is the easiest way to show a colleague or a reviewer what an agent did: open it in any browser, attach it to a PR or an issue. Cyber Code exports only JSON and Markdown (`cyber sessions export --format json|md`). Its HTML is a hosted share artifact, which is P4 and needs an account. Reading a Markdown export loses the structure of tool calls, diffs and reasoning.

## What Changes

- `storage-events`, modified "Session export":
  - `--format html` writes one self-contained HTML file. It has no network requests (no external scripts, styles, fonts or images), escapes all session content, and contains:
    - the header (title, Location, model(s), times, token and cost totals),
    - messages with rendered Markdown,
    - tool calls as collapsible blocks with input, output and unified diffs,
    - reasoning collapsed by default,
    - subagent Sessions as nested sections with `--include-children`,
    - a light/dark theme following `prefers-color-scheme`.
  - `--sanitize` applies to HTML exactly as to JSON.
  - `--output <file>` writes to a file (the default is stdout).
- `tui`, new requirement "Export command" (P1): `/export [html|md|json] [path]` exports the current Session. With no arguments it writes HTML to `./<session-title-slug>-<short-id>.html` and prints the path.

## Non-goals

- Hosting, sharing links or uploading. That stays in `session-sharing` (P4).
- Importing HTML. `cyber sessions import` keeps reading JSON.
- Interactive features beyond collapsing sections and the color scheme. The page carries no JavaScript.
- Embedding images. Image attachments render as a placeholder with media type and size, which keeps files small and avoids leaking screenshots by accident; `--embed-images` is a later option.

## Impact

`crates/cyber-cli` (the HTML renderer and the export flag) and `crates/cyber-tui` (`/export`). No server or API change: export already reads through the store. Docs: the guide and the README Sessions row.
