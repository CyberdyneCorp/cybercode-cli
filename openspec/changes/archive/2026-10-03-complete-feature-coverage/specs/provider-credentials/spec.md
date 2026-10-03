## ADDED Requirements

### Requirement: Command credentials
(P1) A credential of kind `command` (`providers.<id>.api.settings.api_key_command: { command, refresh_seconds? (default 3600), format?: "text" | "json" }`, or stored through `cyber providers login <id> --method command`) SHALL run the command outside the sandbox with the server's environment, and use its trimmed stdout as the API key (`text`) or parse `{ api_key, headers?, expires_at? }` (`json`). The result SHALL be cached until `expires_at` or `refresh_seconds`, never written to disk or logs, and a non-zero exit SHALL mark the provider unavailable with reason `credential_command_failed` and the command's stderr in `cyber doctor`. Project-defined commands SHALL require workspace-trust approval.

#### Scenario: Rotating gateway key
- **WHEN** `providers.corp.api.settings.api_key_command` is `{ command: "vault read -field=key llm/corp", refresh_seconds: 900 }`
- **THEN** requests use the command's output as the key and the command is re-run after 15 minutes
