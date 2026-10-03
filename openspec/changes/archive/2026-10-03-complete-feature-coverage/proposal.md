# Complete feature coverage

## Why

Cyber Code's goal is to have every feature of Claude Code, Codex CLI and OpenCode, plus a backend that lets sessions on different machines cooperate. A gap analysis against the three tools (October 2026) and against the multi-machine goal found features with no requirement at all: a local web client, direct machine-to-machine peering without the hosted Relay, workflow and subagent fan-out across Runners, distributed teams, automation tokens for CI, headless streaming input, several hook events and handler kinds, path-scoped rules, a feature-flag registry, references, model fallback, command credentials, sandbox profiles, MCP server options, and a dozen smaller items.

## What Changes

- New capability `web-client`: local browser UI served by the user's own server without an account (P1), the same bundle served by the Relay (P3), and a desktop shell (P4).
- Multi-machine without the cloud: `peers` config and direct peer transport for messaging and attach (P2); `agent()` and the `agent` tool gain `runner` and `isolation: remote` (P3); distributed teammates (P3); automation tokens for CI (P3).
- Headless parity: `--input-format stream-json`, partial messages and hook events in the stream, `-c key=value` overrides, `--add-dir`, `--allow`/`--deny`, `--system-prompt`, `--append-system-prompt`, `--mcp-config`.
- Permissions: Session ruleset API, auto-mode rules and `/approve`, rule dry run, additional working directories, sandbox profiles and environment policy.
- Context and extensibility: `references`, path-scoped rules and `AGENTS.override.md`, feature-flag registry (`features.*`, replacing `experimental.*`), ten more hook events plus `agent`, `if`, `once` handlers, plugin request interception, MCP `required`/`headers_command`/`output_token_limit`/WebSocket, native provider compaction, websearch backend rotation.
- Models and credentials: fallback chain, `command` credential kind, reasoning display and `/effort`, org effort ceiling.
- Cloud: `cyber apply <session>` and best-of-N attempts. SDK: hook callbacks, per-prompt system prompt and tool options, Python SDK moved to P3.
- Importers updated for Claude Code rules and hook mapping with precedence preservation, and for current Codex configuration.

## Impact

Documentation-only revision of the greenfield target. One new capability, 24 capabilities with ADDED requirements, 9 with MODIFIED requirements. ROADMAP gains dependency A7 (automation tokens) and the Python SDK moves from P4 to P3.
