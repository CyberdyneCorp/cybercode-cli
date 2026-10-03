## MODIFIED Requirements

### Requirement: Built-in tool set
(P0) The system SHALL register these built-in tools in every Location: `read`, `write`, `edit`, `apply_patch`, `glob`, `grep`, `list`, `bash`, `webfetch`, `websearch`, `todo`, `question`, `skill`, `notebook_edit`, `history_search` (compaction), and the hidden `invalid`. It SHALL add `powershell` on Windows, `lsp` when code intelligence is enabled (P1), `monitor` (P1), and `advisor` when `model_roles.advisor` is configured (P2). The registry SHALL offer `apply_patch` instead of `edit`/`write` to models whose catalog entry sets `capabilities.prefers_apply_patch: true`, and `edit`/`write` to all other models.

#### Scenario: GPT-family model gets apply_patch
- **WHEN** the Turn model's catalog entry has `prefers_apply_patch: true`
- **THEN** the advertised tools include `apply_patch` and exclude `edit` and `write`
