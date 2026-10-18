## 1. Status

- [x] 1.1 `idle` status for configured servers that no Session started at the Location; `transport` on status rows. Test: `cyber-app` `mcp` `configured_status_is_read_only_redacted_and_preserves_unknown_ownership`.

## 2. CLI

- [x] 2.1 `cyber mcp list|add|remove` with scopes and comment-preserving edits. Tests: `cyber-cli` `mcp_config_cli`, `cyber-core` `config::edit` unit tests.

## 3. TUI

- [x] 3.1 Always-shown MCP sidebar section with status, transport, tool counts, notes, the six-row cap and the empty state. Tests: `cyber-tui` `screens_mcp` (dark, light, `NO_COLOR`, empty), `screens_sidebar` end to end.
