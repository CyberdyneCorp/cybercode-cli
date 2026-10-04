# Implement P0 operations: backups, logs, retention, doctor

## Why

The P0 exit criteria require a full database-plus-artifact backup that restores and verifies its manifest. The M0.3 notes left retention cleanup, backup bundles and logging for the end of P0. `cyber doctor` is a P0 command.

## What Changes

- `cyber db backup <file>`: an online backup (SQLite backup API, all pages in one step) that runs while the server writes, followed by an integrity check.
- `cyber db backup <dir> --artifacts`: a bundle with the database, tool output, snapshot repositories and `manifest.json` (size and SHA-256 of every file). No partial bundle is ever left behind.
- `cyber db verify <dir>`: checks every manifest entry, reports unexpected files and checks database integrity.
- `cyber db restore <file|dir>`: refuses while a server is registered and verifies bundles first. Current files are kept aside as `*.pre-restore-<id>`.
- `cyber db vacuum`: refuses while a server is registered.
- Structured JSON-lines logs in `<data>/log/cyber-<YYYY-MM-DD>.log`, one file per UTC day with 14 days kept, at `--log-level` / `CYBER_LOG_LEVEL` (default `info`), mirrored to stderr with `--print-logs`. Lines carry `ts`, `level`, `component`, `msg` and, where they apply, `session_id` and `request_id`. Logged events:
  - every HTTP request, with an `x-request-id` header;
  - Drain errors;
  - tool crashes, with their operator-only detail;
  - server start and stop.
- A retention sweep 2 minutes after server start and then every 6 hours. It deletes tool output older than 7 days and archived Sessions older than `storage.retention.archived_days` (default 90); 0 disables a rule.
- `cyber doctor`: pass, warn or fail lines for config, catalog freshness, provider keys (local keyless endpoints listed but not counted), database `quick_check`, sandbox, git, ripgrep and server health; exits 1 on any failure.

## Impact

`cyber doctor` omits the LSP, MCP and account checks until those features exist. Retention covers tool output and archived Sessions; jobs and workflow transcripts follow with those features (P1–P2).

Snapshot repositories reference their project's git objects (git alternates), so a bundle restores snapshots completely only on a machine where those project repositories still exist; objects written by the snapshot itself (untracked and changed files) are inside the bundle.
