## 1. Export route (`storage-events` → Session export)

- [x] 1.1 `GET /sessions/{id}/export`: Session info and totals, messages with parts and tool calls, Goals with evaluations, Workflow Run summaries; `include_children=true` nests child Sessions (archived included), oldest first.
- [x] 1.2 `sanitize=true`: `[redacted:<kind>:<n>]` markers for `text`, `path`, `url`, `tool` and `metadata`; structure, IDs, token counts and timings kept. Unit tests in `http/export/sanitize.rs`.
- [x] 1.3 OpenAPI `v1.session.export` and the regenerated SDK.

## 2. CLI

- [x] 2.1 `cyber sessions export <id> [--sanitize] [--include-children] [--format json|md]`; the Session is named as for `show`; `md` is a global `--format` value.
- [x] 2.2 Markdown: a heading per Session, messages with tool calls, Goals, Workflow runs, child Sessions one level deeper.

## 3. Tests and docs

- [x] 3.1 `sessions_export_as_json_markdown_and_sanitized` (`crates/cyber-cli/tests/sessions_cli.rs`, the real binary): JSON, Markdown, sanitize with totals and timings kept.
- [x] 3.2 `workflow_agent_with_goal_runs_until_check_passes` (`crates/cyber-app/tests/workflows.rs`): run summaries, `include_children` with the child's Goal, sanitize of a nested export.
- [x] 3.3 `docs/cli.md` (`cyber help-markdown`), the local core guide and the README Sessions row.
- [x] 3.4 Tick `implement-m2-3-goals` 6.4 (export) and, where that task exists, `implement-m2-1-workflow-runtime` 5.9 (export of run summaries; finished-transcript retention stays open).
- [ ] 3.5 Archive with `openspec archive add-session-export` after merge.
