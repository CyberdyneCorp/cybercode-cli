## ADDED Requirements

### Requirement: Tool output redaction settings
(P1) The system SHALL read `tool_output.redact` as `{ enabled?: boolean (default true), patterns?: [string], allow?: [string] }` (`tool-registry` → Tool output redaction).

- **`patterns`** are extra regexes, each whole match redacted as `custom`.
- **`allow` entries** are values never redacted: an entry written `/re/` is a regex the whole value must match, and anything else is a literal.

The settings SHALL be validated:
- at most 32 `patterns` and 64 `allow` entries;
- each a non-empty string of at most 1024 bytes;
- each regex compiling within a 256 KiB program and 1 MiB DFA size limit;
- no regex matching empty text;
- no unknown keys.

Invalid settings SHALL fail loading with `tool_output.redact.<key>[<index>]: <reason>`.

A project or plugin layer, trusted or not, and including its `profiles`, SHALL only tighten redaction. Its `enabled` (unless `true`) and `allow` SHALL be dropped with a warning, and its `patterns` SHALL apply. `tool_output.redact.patterns` and `tool_output.redact.allow` SHALL concatenate across layers, so no layer can drop another's entries.

#### Scenario: Project cannot disable redaction
- **WHEN** a trusted project's `cyber.jsonc` sets `tool_output.redact.enabled: false`, `allow: ["/.+/"]`, a pattern, and a default profile that sets `enabled: false`
- **THEN** redaction stays on, the allow list holds only the user's entries, the user's and the project's patterns both apply, and a warning names the ignored settings

#### Scenario: Invalid pattern
- **WHEN** config sets `tool_output.redact.patterns: ["(unclosed"]`
- **THEN** loading fails with an issue starting `tool_output.redact.patterns[0]:`
