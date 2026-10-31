## 1. Detection module

- [x] 1.1 Add `crates/cyber-core/src/redact/` with the shared detectors (moved from `memory/secrets.rs`, one kind each), known-value collection, `RedactSettings` and the `Redactor` engine. The engine:
  - scans once per detector and builds the spans;
  - orders them by start, then longest, then priority, and removes overlaps;
  - rebuilds the text once, and returns it borrowed when nothing was found;
  - keeps the line breaks of a multi-line match;
  - skips existing markers, so redacting is idempotent.
- [x] 1.2 Make `memory::redact_credentials(_with)` and the memory `check` delegate to the module with the permissive legacy filter, and keep the high-entropy check in memory.
- [x] 1.3 Add `EnvSource::vars` for the process environment and `HashMap`.
- [x] 1.4 Unit tests:
  - every provider format;
  - PEM blocks with and without the `END` line;
  - assignments, with false-positive guards for code, placeholders, numbers and identifiers;
  - hashes, UUIDs, base64 images and lockfile hashes kept unchanged;
  - tokens after JSON escapes;
  - exact env values, without the value in `Debug`;
  - custom patterns, allow entries and the switch;
  - overlap priority and idempotence;
  - `introduced_marker`, `added_markers` and the legacy markers;
  - 1 MiB under 10 ms in release and 50 ms in debug, best of 5.
- [x] 1.5 Build `regex`, `regex-automata`, `regex-syntax`, `aho-corasick` and `memchr` at `opt-level = 3` in dev builds.

## 2. Configuration

- [x] 2.1 Validate `tool_output.redact` in `settings_issues`: limits, regex size, empty matches and unknown keys.
- [x] 2.2 In the loader's per-layer step, drop `enabled` (unless `true`) and `allow` from `project:` and `plugin:` layers and their profiles, with a warning. Concatenate `patterns` and `allow` across layers.
- [x] 2.3 Tests:
  - a trusted and an untrusted project, plus a default profile, cannot disable redaction or allow values, and the user's and the project's patterns both apply;
  - the user can disable it;
  - an invalid pattern fails loading.

## 3. Projection and settlement

- [x] 3.1 `Budget` carries a `Redaction`. `truncate` redacts the raw text, measures and cuts the redacted text, and stores the raw text in the overflow file.
- [x] 3.2 The host builds the Location's `Redactor` from settings, secret-named env vars, secret config entries and `ModelResolver::credential_values`, caching the last one. It notes per-call counts from `settle_output` and `bound_registered_output`.
- [x] 3.3 `read` redacts the whole file before paging and notes the markers new to the page.
- [x] 3.4 Add `ToolHost::redact_model_text` and `take_call_redactions`, with built-in defaults, implemented by `BuiltinHost` and forwarded by `AppHost`. `with_call_metadata` redacts the settled output and writes `redactions`, both for a normal group and for an auto-override replay.
- [x] 3.5 Job notices and workflow summaries redact their strings before JSON encoding, and session import redacts call outputs.
- [x] 3.6 Add `redactions` to `ToolSettled` and `CallState`. Regenerate `sdk/openapi.json` and the SDK types.
- [x] 3.7 Integration tests (`cyber-tools/tests/redaction.rs`, `mcp_runtime.rs`, `background_bash.rs`):
  - bash `echo $SECRET`, a `.npmrc` read, exact env and provider values, and an overflowed result with a key in its tail all reach the model redacted, with the model's results equal to the stored settlements and the `redactions` counts on both;
  - the overflow file is raw, and a re-read is redacted, including a page that starts inside the key;
  - an MCP result with a token and a server `env` secret;
  - a background Job notice is redacted while its log stays raw;
  - user patterns, allow entries and the switch.

## 4. Writes

- [x] 4.1 `guarded_write` (`write`, `edit`, `notebook_edit`) and `apply_patch` refuse introduced markers before asking for permission. `bash`, `powershell`, `pty_start` and `pty_write` refuse markers. `edit` explains a marker in `old_string`.
- [x] 4.2 Tests:
  - `write`, `edit`, `apply_patch` and `bash` refuse, and the file is unchanged;
  - a marker in `old_string` gets the hint;
  - a file that already holds a marker can still be edited.

## 5. Model guidance and clients

- [x] 5.1 Add the marker guidance to `prompts/base/core.md`, bump the base prompt to version 3, and cover the topic in the prompt test.
- [x] 5.2 TUI: `Tool.redactions`, with the `⊘ n secrets hidden from the model` line and a render test.
- [x] 5.3 Dashboard: `redactionNote` and the badge, with a unit test, and rebuild `web/dist`.
- [x] 5.4 `exec`: `redactions` on `tool_result`, the stderr note in text mode, and a unit test.

## 6. Documentation and gate

- [x] 6.1 `docs/local-core-guide.md`, `docs/features.md` and `ROADMAP.md`.
- [x] 6.2 Run the gate:
  - fmt and clippy;
  - the sandbox helper build;
  - nextest;
  - the OpenAPI and SDK checks, and `help-markdown`;
  - the web tests and build check;
  - OpenSpec validation and `spec_lint`.
