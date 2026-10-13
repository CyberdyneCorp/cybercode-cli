## Why

M1.5 requires reviewed adoption from Claude Code, Codex and OpenCode. Editor integration moved to P3 (`defer-editor-integration`). Read-time compatibility exists, but the CLI has no import command. Conversion must preserve permission precedence before any proposed configuration can be written.

## What Changes

- Implement every P1 compat-import contract: source discovery, conversion, project/global scopes, ordered auto import, reviewable dry-run diffs and confirmation, mapping reports, secret safety, detection and idempotent re-import.
- Editor integration (ACP, VS Code) moved to P3 by `defer-editor-integration`; this change no longer delivers it.
- Begin with a pure Claude permission/mode converter and test the emitted rules through the existing permission engine. This foundation does not write files or imply command/milestone acceptance.
- Keep P2 session transcript import distinct from required P1 configuration import; report unsupported requests explicitly when the CLI is implemented.

## Capabilities

### Modified Capabilities
- `compat-import`: deliver P1 migration and reporting.

## Impact

Core import data/parsers, CLI source discovery/review/writes, trust and credentials, session/runtime editor adapters and editor clients. Preserve P0 evaluation/services/backups and the full active P1 objective. No milestone is accepted by this first increment.
