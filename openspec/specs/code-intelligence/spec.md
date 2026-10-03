# code-intelligence Specification

## Purpose
Gives the agent compiler-grade feedback and navigation through Language Server Protocol clients, and keeps agent-written files in project style through formatters. Diagnostics are fed back to the model after edits so it fixes errors in the same Turn. Draws on OpenCode v1's built-in LSP and formatter integration, and Claude Code's `LSP` tool and code-intelligence plugins.

## Requirements

### Requirement: LSP enablement
(P1) When `lsp` is omitted, the system SHALL enable every built-in server whose executable is found on PATH or in `<cache>/bin`. `lsp: false` SHALL disable all servers. `lsp: { <id>: {...} }` SHALL apply per-server overrides on top of the auto-detected set. Downloading missing servers SHALL require `lsp.auto_install: true` (default `false`).

#### Scenario: Auto-detected server
- **WHEN** `rust-analyzer` is on PATH and `lsp` is omitted
- **THEN** Rust files activate the `rust-analyzer` server

#### Scenario: No downloads by default
- **WHEN** `gopls` is missing and `auto_install` is false
- **THEN** no download occurs and `cyber doctor` lists `gopls: not installed`

### Requirement: Built-in servers
(P1) The system SHALL ship definitions (id, extensions, root markers, launch command, install method) for at least: `rust-analyzer`, `typescript` (tsserver via typescript-language-server), `pyright`, `gopls`, `clangd`, `jdtls`, `lua-language-server`, `zls`, `bash-language-server`, `yaml-language-server`, `svelte`, `vue`, `solidity`, and `verible` (SystemVerilog).

#### Scenario: Server list
- **WHEN** the user runs `cyber lsp status`
- **THEN** each built-in server is listed with `enabled`, `installed` and `running` columns

### Requirement: Custom and overridden servers
(P1) An `lsp.<id>` entry SHALL accept `command` (string array), `extensions`, `root_markers`, `env`, `initialization_options` and `disabled`. Custom ids SHALL require `extensions`, or config validation fails.

#### Scenario: Custom server
- **WHEN** config defines `lsp.nimlsp` with `command: ["nimlangserver"]` and `extensions: [".nim"]`
- **THEN** `.nim` files activate it

### Requirement: Lazy spawning and root detection
(P1) The system SHALL spawn a server lazily the first time a matching file inside the Location is read or edited. The root SHALL be found by walking up to the nearest `root_markers` match, else the Location. Concurrent spawns for the same server and root SHALL be deduplicated. If the initialize handshake does not complete within 45 s, the server and root SHALL be marked broken for the lifetime of that Location's services.

#### Scenario: Broken server not retried
- **WHEN** `pyright` fails to initialize
- **THEN** it is not respawned for that root until the Location reloads

### Requirement: Diagnostics after edits
(P1) After `edit`, `write`, `apply_patch` or `notebook_edit` changes a file, the system SHALL notify the server (`didOpen`/`didChange`/`didSave`) and wait up to `lsp.diagnostics_wait_ms` (default 5000, debounced 150 ms) for fresh diagnostics. It SHALL append only error-severity diagnostics to the tool output as `<diagnostics file="<path>">` blocks, at most 20 per file followed by `… and <n> more`, each `ERROR [<line>:<col>] <message>` with 1-based positions. `write` SHALL also report new errors in up to 5 other files.

#### Scenario: Type error fed back
- **WHEN** an edit introduces a type mismatch
- **THEN** the tool result ends with a `<diagnostics>` block containing the error

### Requirement: lsp tool
(P1) The `lsp` tool SHALL accept `{ operation: definition|references|hover|document_symbols|workspace_symbols|implementation|rename_preview|diagnostics, path?, line?, character?, query?, new_name? }` with 1-based positions and check the `lsp` permission. `rename_preview` SHALL return the proposed edits without applying them. Results SHALL be capped at 50 locations, with paths relative to the Location.

#### Scenario: Find references
- **WHEN** the model requests references at `src/lib.rs:10:5`
- **THEN** up to 50 locations are returned as `path:line:col` with a code preview

### Requirement: Read warms servers
(P1) `read` on a file with an applicable server SHALL open the document in the background without waiting for diagnostics.

#### Scenario: Warm start
- **WHEN** the model reads a `.ts` file
- **THEN** the TypeScript server starts in the background and the read is not delayed

### Requirement: LSP status and shutdown
(P1) The server SHALL expose `GET /api/v1/lsp` with `{ id, root, status: starting|connected|broken }` entries. Every LSP process SHALL be shut down (`shutdown`/`exit`, then kill after 3 s) when the Location's services close.

#### Scenario: Shutdown on close
- **WHEN** a Location is idle for 60 minutes and its services are released
- **THEN** its LSP processes exit

### Requirement: Formatter enablement and detection
(P1) When `formatters` is omitted, the system SHALL enable each built-in formatter whose detection succeeds: its binary is on PATH and, where defined, a project marker exists. Built-ins SHALL include `rustfmt`, `prettier`, `biome`, `ruff`, `black`, `gofmt`, `clang-format`, `shfmt`, `stylua`, `zig fmt`, `forge fmt` and `verible-verilog-format`. `formatters: false` SHALL disable all of them.

#### Scenario: Prettier requires config
- **WHEN** `prettier` is on PATH but the project has no prettier config or dependency
- **THEN** prettier is not enabled

### Requirement: Custom formatters
(P1) A `formatters.<id>` entry SHALL accept `command` (string array with `$FILE` replaced by the absolute path), `extensions`, `env` and `disabled`. An entry with `command` SHALL always be enabled for its extensions.

#### Scenario: Custom formatter command
- **WHEN** `formatters.taplo` has `command: ["taplo", "fmt", "$FILE"]` and `extensions: [".toml"]`
- **THEN** edited `.toml` files are formatted with taplo

### Requirement: Formatter execution
(P1) After a file edit, the system SHALL run every enabled formatter matching the extension sequentially, in the Location directory, inside the sandbox, with a 30 s timeout each. Failures and non-zero exits SHALL be logged and SHALL NOT fail the tool call. When a formatter changed the file, the tool result SHALL reflect the formatted content and diff.

#### Scenario: Formatter failure tolerated
- **WHEN** `ruff format` exits 2 on a syntax error
- **THEN** the edit succeeds and the log records the formatter failure

### Requirement: Formatter status
(P1) The server SHALL expose `GET /api/v1/formatters` returning `{ id, extensions, enabled, detected_by }`, and the CLI SHALL provide `cyber fmt status`.

#### Scenario: Formatter status listing
- **WHEN** a client requests formatter status
- **THEN** each formatter is returned with whether detection currently succeeds

### Requirement: Code-intelligence plugins
(P4) Plugins SHALL be able to contribute LSP server definitions and formatter definitions through the plugin manifest (`lsp_servers`, `formatters`). These SHALL merge with built-ins, and user config overrides SHALL take precedence.

#### Scenario: Plugin adds a language
- **WHEN** an installed plugin contributes an `ocamllsp` definition
- **THEN** `.ml` files activate it unless user config disables it
