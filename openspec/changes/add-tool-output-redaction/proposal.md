## Why

Secrets that appear in tool output reach the model provider unredacted. A `bash` `env` or `printenv`, a build log, a `cat` of `~/.aws/credentials` or `.npmrc`, an MCP result or a fetched page that contains an API key is sent to the provider as is, and it is stored in the Session, so the TUI, the web dashboard, exports and replay show it too.

Permissions only gate reading `*.env` files, which is one path among many. `provider-credentials` → Redaction already promises that credential values are removed from "tool output shown to the model", but nothing implements it.

A credential detector already exists in `crates/cyber-core/src/memory/secrets.rs`. It is used for memory writes, git text, telemetry and exports, but not for tool output.

## What Changes

- **Tool output redaction** (`tool-registry`, added "Tool output redaction"). The text of every tool result is redacted before the model sees it. This covers built-in tools (`read`, `grep`, `bash`/`powershell`, Job output reads, `webfetch`/`websearch`), MCP, browser and plugin tools, subagent results, Job completion notices and workflow run summaries.
  - The registry projection redacts the raw result **before** truncation, so a secret is hidden whole even where the cut falls, and the overflow preview is redacted too. A later untrusted-content frame wraps the redacted, truncated text.
  - The runtime redacts the settled output once more (failures, reminders), and that one text is both what the model gets and what `session.tool.settled` stores. The TUI, web, exports and replay therefore never show more than the model saw.
  - Raw overflow files and Job logs stay on this machine unredacted. Reading them back through a tool redacts them again.
- **Detection** (`tool-registry`, added "Secret detection"):
  - (a) Credential formats: AWS access keys, GitHub (`ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_`), OpenAI (`sk-`, `sk-proj-`), Anthropic (`sk-ant-`), Slack (`xox[abposr]-`), Stripe live keys (`sk_live_`, `rk_live_`), Google (`AIza`), GitLab (`glpat-`), npm (`npm_`), JWTs, PEM `PRIVATE KEY` blocks through their `END` line, `Authorization` header values, URL passwords, and the existing Copilot and peer-grant tokens.
  - (b) Secret-looking assignments: `NAME=value` and `"name": "value"` where the name contains `token`, `secret`, `password`, `api_key`, `credential`, `private_key` and similar, and the value is at least 8 characters and does not look like code, a placeholder or a number.
  - (c) Exact values of at least 8 characters of known secrets: secret-named environment variables of the `cyber` process, configured and resolved provider API keys, and secret config entries such as MCP server `env` and `headers`.
  - No generic entropy-based redaction, because hashes, IDs and encoded media are legitimately random.
  - One detector list in `cyber_core::redact` serves tool output, memory, git text, telemetry and exports.
- **Marker** (`tool-registry`, added "Redaction markers"). Each match becomes `[REDACTED:<kind>]`, such as `[REDACTED:github_token]`. A multi-line secret keeps its line breaks, so line numbers do not move. The base prompt explains the marker (`system-context`, added "Redacted secret guidance").
- **Writes refuse markers** (`builtin-tools`, added "Redaction markers are never written"). `write`, `edit`, `apply_patch`, `notebook_edit` and shell input (`bash`, `powershell`, `pty_start`, `pty_write`) refuse content that holds a marker the file does not already hold. The error tells the model that the value is hidden. This stops the model from overwriting a real secret with its placeholder.
- **Settings** (`configuration`, added "Tool output redaction settings"). The `tool_output.redact` key holds `{ enabled: true, patterns: [], allow: [] }`.
  - The regexes are validated and bounded in size.
  - Project and plugin layers may only add `patterns`. Their `enabled: false` and `allow` entries are dropped with a warning, even in a trusted checkout, and `patterns` concatenate across layers so no layer drops another's patterns.
- **Visibility.** `session.tool.settled` and the Session's calls carry `redactions: {count, kinds}`, so clients can say what was hidden (`tui`, `web-client` and `exec-mode`, each one line or badge). OpenAPI and the TypeScript SDK gain `CallState.redactions` and `Redactions`.
- **`provider-credentials` → Redaction** (modified) now points tool output at this marker instead of `***`.

## Non-goals

- **Tool input** that the model wrote is not redacted. The model can only write what it saw, which was already redacted, or what the user gave it.
- **User-provided context**, such as typed prompts, pasted text, `@file` and MCP resource mentions, and `!` shell commands the user typed, is the user's explicit choice and is not redacted. The output of a `!` command is redacted like any shell output.
- **Fragments that lost their shape** cannot be detected without entropy heuristics. Examples are a private key's body lines printed without its `BEGIN` line (`sed -n 2,5p key.pem`) and a token that a tool cut short.
- **Structured values** (`structured_output`, the typed value of a call) are not model content and are not redacted. Workflow scripts' direct tool values are not redacted either; what they hand back to the model is.
- **OAuth tokens in the MCP credential store and subscription logins** are not added as exact values. Their minted forms are covered by the patterns where a pattern exists.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `tool-registry`: added "Tool output redaction", "Secret detection", "Redaction markers" and "Redaction visibility".
- `builtin-tools`: added "Redaction markers are never written".
- `configuration`: added "Tool output redaction settings".
- `system-context`: added "Redacted secret guidance".
- `provider-credentials`: modified "Redaction". No open change modifies this requirement.
- `tui`: added "Redaction notice on tool cards".
- `web-client`: added "Redaction badge on tool results".
- `exec-mode`: added "Redaction notes in exec output".

## Impact

- `crates/cyber-core/src/redact/`: the new module: detectors, known values, settings and the engine. `memory::secrets` now delegates to it.
- `crates/cyber-core/src/config/`: validation, the project-layer restriction and concatenated patterns. The base prompt moves to version 3.
- `crates/cyber-tools`:
  - `Budget` redacts before it truncates;
  - the host builds and caches the Location's redactor and notes per-call counts;
  - `read` redacts the whole file before paging it;
  - the write tools and shell input refuse markers.
- `crates/cyber-server`:
  - `ToolHost::redact_model_text` and `take_call_redactions`, with built-in defaults;
  - `ModelResolver::credential_values`;
  - settlement redaction, and redacted Job and workflow handbacks and session imports;
  - the `redactions` field on events and `CallState`.
- TUI, web dashboard, `exec`, `sdk/openapi.json` and the generated SDK types.
- This change composes with `add-untrusted-content-defense`. Redaction runs inside the registry's truncation step on the raw text, and framing wraps its result.
