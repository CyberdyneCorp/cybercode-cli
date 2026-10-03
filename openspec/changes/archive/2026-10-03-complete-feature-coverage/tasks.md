## 1. Multi-machine and backend

- [x] 1.1 `web-client` capability (local web UI, Relay bundle, desktop shell).
- [x] 1.2 Direct peer transport and `peers` config (cross-session-messaging, configuration, cli-commands).
- [x] 1.3 Remote fan-out: `runner` for workflow agents and `isolation: remote` for the agent tool; distributed teammates.
- [x] 1.4 Automation tokens (cyber-account) and ROADMAP dependency A7.

## 2. Headless and CLI parity

- [x] 2.1 Streaming input and partial output for `exec`; `--output-last-message`.
- [x] 2.2 Global `-c`, `--add-dir`, `--allow`/`--deny`, `--system-prompt`, `--append-system-prompt`, `--mcp-config`, `--features`.
- [x] 2.3 Commands `web`, `peers`, `tokens`, `features`, `apply`, `permissions test`.

## 3. Permissions, sandbox, context, extensibility

- [x] 3.1 Session ruleset API, auto-mode rules and `/approve`, rule dry run, additional working directories.
- [x] 3.2 Sandbox profiles and environment policy.
- [x] 3.3 Features registry, `references`, path-scoped rules, `AGENTS.override.md`, instruction size cap.
- [x] 3.4 Hook events and handler kinds; plugin request interception; MCP server options and WebSocket; native compaction; websearch rotation.

## 4. Models, credentials, cloud, SDK, import

- [x] 4.1 Model fallback chain, `command` credentials, reasoning display, `/effort`, org effort ceiling.
- [x] 4.2 `cyber apply`, best-of-N attempts.
- [x] 4.3 SDK hook callbacks and prompt options; Python SDK to P3.
- [x] 4.4 Importer updates for Claude Code and Codex.
- [x] 4.5 README capability map, ROADMAP phases and inventory.
