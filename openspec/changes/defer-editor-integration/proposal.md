## Why

Product decision of 2026-10-13: P1 should close on skills, hooks and core parity with Claude Code, Codex and OpenCode, not on IDE clients. Editor integration (ACP, the VS Code extension, IDE context and IDE detection) is a set of clients of the local server, and it belongs with the other P3 connectivity clients (remote control, the Relay web client, sharing). Keeping its 13 P1 requirements blocks P1 acceptance and the M1.5 milestone without improving the core.

The migration path (`cyber import claude|codex|opencode`, `compat-import`) is unaffected and stays in P1 and M1.5.

## What Changes

- `editor-integration`: these 13 requirements move from P1 to P3, with text and scenarios unchanged:
  - ACP command and transport
  - ACP initialize and capabilities
  - ACP session lifecycle
  - ACP config options
  - ACP prompting and streaming
  - ACP tool calls and permissions
  - ACP file write-through
  - ACP client MCP servers
  - VS Code extension
  - Inline diff review
  - Selection mentions and plan review
  - IDE context source
  - IDE detection and extension install
- "Deep links" stays P3 and "JetBrains plugin" stays P4.
- ROADMAP: M1.5 becomes the migration path only. ACP for Zed and JetBrains and the VS Code extension join P3, with milestone M3.7 Editor clients.
- `implement-m1-5-migration-editor` keeps its editor tasks, annotated as moved to P3. No implementation exists to remove.

No other capability needs a change. The `acp` and `ide` commands (`cli-commands` "Command tree"), the `ide` route group (`server-api` "Route groups") and the `ide` configuration key (`configuration` "Top-level keys") are P0 registries. They already return an unavailable-capability error until the owning phase ships.

## Capabilities

### Modified Capabilities
- `editor-integration`: 13 requirements re-phased from P1 to P3.

## Impact

- Specification and roadmap only. No code changes.
- The P1 acceptance scope drops by 13 requirements, and the P1 acceptance audit removes the editor-integration checklist.
- Canonical inventory after archive: P1 −13, P3 +13 (P1 227 → 214, P3 145 → 158). Until this change is archived, the CI-generated inventory table in ROADMAP.md keeps the canonical counts.
