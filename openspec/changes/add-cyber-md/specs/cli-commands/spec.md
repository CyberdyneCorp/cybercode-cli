## MODIFIED Requirements

### Requirement: Doctor and debug commands
(P0) `cyber doctor` SHALL check and report, as pass/warn/fail lines: config parse and validation, provider credentials, model catalog freshness, database integrity (`PRAGMA quick_check`), sandbox availability, ripgrep and git availability, LSP binaries, MCP connectivity, server health, account token validity, and instruction files. The `instructions` line SHALL list, by source path and in load order, the instruction files (`AGENTS.md`, `CLAUDE.md`, `CONTEXT.md`, `CYBER.md`, overrides and imports) that a new Session at the Location loads. It SHALL read `compat.claude` from the same per-Location config as the runtime, and SHALL warn about truncated or left-out files, files outside the project, not regular or refused by the `read` permission rules, and unresolved or refused imports. When the Location's config fails to load it SHALL list no project files, as the runtime loads none. It SHALL exit 1 when any check fails. `cyber debug` SHALL expose `config` (resolved, secrets redacted), `paths`, `info`, `agent <name>`, `tool <name> --params <json>`, `context` (rendered system context) and `events <session>`.

#### Scenario: Doctor detects missing sandbox dependency
- **WHEN** the user runs `cyber doctor` on Linux without `bwrap` installed
- **THEN** the sandbox line reports `fail: bubblewrap not found; install bubblewrap or set sandbox.enabled=false`
- **AND** the exit code is 1

#### Scenario: Debug config redaction
- **WHEN** config contains `providers.openai.api_key = "sk-abc"` and the user runs `cyber debug config`
- **THEN** the printed value is `"***"`

#### Scenario: Doctor lists CYBER.md
- **WHEN** the project contains `AGENTS.md` and a `CYBER.md` larger than `instructions.max_bytes`, and the user runs `cyber doctor`
- **THEN** the instructions line lists `AGENTS.md` and then `CYBER.md` by path, and warns that `CYBER.md` was truncated
