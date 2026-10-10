## ADDED Requirements

### Requirement: Owned LSP connection lifecycle
(P1) LSP connections SHALL consume already-authorized processes, initialize within a default 45-second deadline and retain ownership until explicit native settlement. Closing SHALL attempt shutdown/exit for at most three seconds before forced tree termination and separate acknowledgement. Timeout or cancellation SHALL NOT permit request replay or imply termination.

#### Scenario: Initialized connection
- **WHEN** an authorized LSP process successfully responds to initialize
- **THEN** the client SHALL send initialized and retain its capabilities, immutable workspace root and bounded untrusted notifications
- **AND** unsupported server requests SHALL NOT authorize workspace mutations

#### Scenario: Failed or interrupted connection
- **WHEN** initialization fails or a request is interrupted
- **THEN** incomplete protocol ownership SHALL remain fenced and cleanup SHALL terminate the owned tree, retaining the distinction between forced termination and native acknowledgement
- **AND** server response and stderr content SHALL NOT appear in error display

#### Scenario: Owned shutdown
- **WHEN** shutdown succeeds or stalls
- **THEN** the connection SHALL settle the owned process tree, including descendants after leader exit, or report acknowledgement as unavailable
- **AND** dropping an owner SHALL terminate its tree without claiming settlement

### Requirement: Bounded LSP transport framing
(P1) LSP stdio SHALL use bounded ASCII Content-Length headers and UTF-8 JSON-RPC 2.0 object bodies. Interrupted or failed transport I/O SHALL fence both directions without claiming native termination or granting launch authority.

#### Scenario: Exact byte framing
- **WHEN** adjacent or fragmented LSP messages contain multibyte Unicode
- **THEN** the transport SHALL consume exactly each declared content byte length and preserve message boundaries
- **AND** headers above 8 KiB, content above 4 MiB, duplicate or invalid lengths, unsupported charsets and malformed messages SHALL be refused with diagnostics excluding server content

#### Scenario: Cancelled partial transport I/O
- **WHEN** a read or write future is cancelled after transport admission
- **THEN** both subsequent reads and writes SHALL be refused rather than resuming or replaying a partial frame
- **AND** the caller SHALL retain responsibility for native process termination and settlement

## MODIFIED Requirements

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

### Requirement: Custom formatters
(P1) A `formatters.<id>` entry SHALL accept `command` (string array with `$FILE` replaced by the absolute path), `extensions`, `env` and `disabled`. An entry with `command` SHALL always be enabled for its extensions.

#### Scenario: Custom formatter command
- **WHEN** `formatters.taplo` has `command: ["taplo", "fmt", "$FILE"]` and `extensions: [".toml"]`
- **THEN** edited `.toml` files are formatted with taplo


#### Scenario: Typed integration configuration
- **WHEN** trusted resolved configuration is loaded
- **THEN** `lsp` and `formatters` SHALL accept omission, false or an object and SHALL reject other section types
- **AND** server and formatter commands SHALL be nonempty string arrays with a nonempty executable, extensions and root markers SHALL contain nonempty strings, and environments SHALL contain string values
- **AND** `lsp.auto_install` SHALL be boolean with default false and `lsp.diagnostics_wait_ms` SHALL be a nonnegative integer with default 5000
- **AND** parsing SHALL NOT start processes, download executables or establish diagnostics authority

#### Scenario: Untrusted integrations remain inactive
- **WHEN** a project defines LSP or formatter commands without approval for its current sensitive digest
- **THEN** those definitions SHALL remain absent from resolved executable configuration
- **AND** approving and subsequently changing the definitions SHALL require fresh trust review

#### Scenario: Local server catalogue and confined root discovery
- **WHEN** language-server definitions are resolved for a Location
- **THEN** built-ins SHALL retain their canonical IDs, extensions, root markers, launch arguments and install methods, with trusted user overrides taking precedence
- **AND** executable discovery SHALL search the supplied PATH followed by `<cache>/bin`, refuse non-executable files on Unix and never run candidate executables
- **AND** root discovery SHALL select the nearest existing root marker without ascending above the canonical Location or accepting files outside it
- **AND** missing executables SHALL remain uninstalled without downloading during discovery

### Requirement: Formatter enablement and detection
(P1) When `formatters` is omitted, the system SHALL enable each built-in formatter whose detection succeeds: its binary is on PATH and, where defined, a project marker exists. Built-ins SHALL include `rustfmt`, `prettier`, `biome`, `ruff`, `black`, `gofmt`, `clang-format`, `shfmt`, `stylua`, `zig fmt`, `forge fmt` and `verible-verilog-format`. `formatters: false` SHALL disable all of them.

#### Scenario: Prettier requires config
- **WHEN** `prettier` is on PATH but the project has no prettier config or dependency
- **THEN** prettier is not enabled


### Requirement: Formatter status
(P1) The server SHALL expose `GET /api/v1/formatters` returning `{ id, extensions, enabled, detected_by }`, and the CLI SHALL provide `cyber fmt status`.

#### Scenario: Formatter status listing
- **WHEN** a client requests formatter status
- **THEN** each formatter is returned with whether detection currently succeeds


#### Scenario: Local status without integration effects
- **WHEN** `cyber lsp status` or `cyber fmt status` inspects the current Location
- **THEN** it SHALL expose every built-in and configured integration from trusted resolved settings without starting a model, creating a database, downloading or running an integration
- **AND** formatter discovery SHALL require a local executable and any built-in project marker, while configured commands SHALL enable their extensions unless explicitly disabled
- **AND** Prettier SHALL detect its supported config filenames, package.json configuration or declared dependency using bounded regular no-follow reads without evaluating configuration code

#### Scenario: Authenticated Location formatter status
- **WHEN** a client requests `GET /api/v1/formatters`
- **THEN** the server SHALL authenticate the request and return Location-scoped formatter ids, extensions, enabled and detected_by fields from fresh trust-filtered configuration and shared discovery
- **AND** query Location SHALL take precedence over header Location, with invalid Locations refused before discovery
- **AND** status SHALL NOT execute candidates, evaluate project configuration code or publish formatter environment/command secrets
- **AND** unavailable service hosts SHALL return a typed service-unavailable error rather than an empty success list
