## Why

`storage-events` → Session export (P0) specifies `cyber sessions export <id> [--sanitize] [--include-children] [--format json|md]`, but the command was listed as not available. Goals (`implement-m2-3-goals` 6.4) and Workflow runs (`implement-m2-1-workflow-runtime` 5.9) both depend on the export carrying their records.

## What Changes

- `GET /api/v1/sessions/{id}/export?sanitize=&include_children=` (`v1.session.export`) returns the export: format `version` 1, `exported_ms`, `sanitized`, the Session info and totals, every message with its parts and tool calls, every Goal with its evaluations, the Workflow Run summaries the Session started, and with `include_children=true` the same for each child Session, recursively.
- `sanitize=true` replaces text, paths, URLs, tool inputs and outputs and metadata with `[redacted:<kind>:<n>]` markers. Equal values keep equal markers; IDs, statuses, kinds, sources, agents, models, modes, numbers, booleans and the shape of the document are kept.
- `cyber sessions export` calls the route and writes JSON (default) or Markdown (`--format md`); `md` joins the global `--format` values and other formats are a usage error.
- The SDK gains `client.sessions.export()` from the OpenAPI operation.
- `--all-branches` (`branches=all`), which `add-session-branches` gave the earlier JSON-only export, is kept on this command: the document carries `branches` and every message's `parent_message_id`, and `--sanitize` keeps both.

## Impact

- Specs: `storage-events` (Session export) gains the route, the document layout and the marker rules (spec delta).
- Code: `crates/cyber-server/src/http/export.rs` and `export/sanitize.rs`, `crates/cyber-cli/src/commands/sessions.rs` and `session_export.rs`, the global `Format`.
- Out of scope: `cyber sessions import` (its `ExportTooNewError` uses the `version` field), share URLs.
