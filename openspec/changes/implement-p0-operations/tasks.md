## 1. Backups

- [x] 1.1 Online backup and integrity check in `cyber-store`; consistency test under concurrent writes.
- [x] 1.2 Bundles with manifests; verify; restore with server refusal and kept copies; tests.
- [x] 1.3 `cyber db backup|verify|restore|vacuum`.

## 2. Logs, retention, doctor

- [x] 2.1 JSON-lines logger with daily files, 14-day pruning, levels, stderr mirroring; request, error, crash and lifecycle logging.
- [x] 2.2 Retention sweep for tool output and archived Sessions; tests with a simulated clock.
- [x] 2.3 `cyber doctor` with JSON output and exit codes.
- [ ] 2.4 Doctor checks for LSP, MCP and account tokens; retention for jobs and workflow transcripts (with those features).
