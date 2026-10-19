## MODIFIED Requirements

### Requirement: Component discovery defaults
(P1) When a component path is omitted, the system SHALL look for conventional locations inside the plugin root: `skills/*/SKILL.md`, `commands/**/*.md`, `agents/**/*.md`, `hooks/hooks.json`, `.mcp.json`, `workflows/*.{js,ts}`, `themes/*.json`. Components SHALL be namespaced as `<plugin>:<component>` when they would collide with an existing name; a plugin component SHALL NOT replace a built-in, user or project component. MCP server names cannot contain `:`, so a colliding plugin MCP server SHALL be named `<plugin>-<server>`. `${CYBER_PLUGIN_ROOT}` in hook and MCP definitions SHALL expand to the plugin root.

#### Scenario: Namespaced command on collision
- **WHEN** plugin `review-kit` provides command `review` and a built-in `review` exists
- **THEN** the plugin command is available as `/review-kit:review`

#### Scenario: Namespaced MCP server on collision
- **WHEN** plugin `review-kit` ships a `.mcp.json` server `docs` and the user configuration already defines `mcp.docs`
- **THEN** the plugin server is available as `review-kit-docs` and the user's `docs` is unchanged

### Requirement: Pure mode
(P1) The global `--pure` flag and `CYBER_PURE=1` SHALL disable all non-built-in plugins, project hooks and project MCP servers for that process while keeping built-ins. Hooks and MCP servers SHALL then load only from organization policy. Because a shared server cannot apply pure mode to one client, `cyber exec`, `cyber review` and the TUI SHALL run an in-process server under pure mode, and commands that can only start Turns on the shared server (`cyber api`, `cyber pr`, `cyber mcp serve`, `cyber workflows run` and `resume`) SHALL be refused.

#### Scenario: Pure mode run
- **WHEN** the user runs `cyber --pure`
- **THEN** no external plugin host is spawned and `/plugins` shows them as disabled by `--pure`

#### Scenario: Only managed hooks under pure mode
- **WHEN** global, project, plugin and managed hooks match a call and the user runs `cyber exec --pure`
- **THEN** only the managed hook runs

### Requirement: Plugin host process
(P1) Plugins that declare `tools`, `providers`, `hooks.runtime` or `channels` code SHALL run in a dedicated host process per plugin, launched from the manifest `main` with the declared `runtime` (`node`, `bun`, `deno`, `python` or a native executable). The host SHALL communicate over JSON-RPC 2.0 on stdio. The core SHALL never load plugin code in-process. Because the sandbox grant depends on the Location, a host process SHALL serve one plugin in one Location. The host SHALL always run sandboxed, whatever the Session's sandbox policy; where no sandbox enforcement is available, the host SHALL NOT start and the plugin SHALL report why. A plugin's MCP servers SHALL run sandboxed to the same declared capabilities. A plugin tool's `readOnlyHint` SHALL count for plan mode and permission auto-allow only when the plugin declares no `fs.write`, `network`, `shell` or `session.prompt` capability.

#### Scenario: Plugin crash isolated
- **WHEN** a plugin host process exits unexpectedly during a tool call
- **THEN** that call fails with `plugin <name> crashed` and the session continues

#### Scenario: No sandbox, no host
- **WHEN** a plugin with `main` is loaded on a machine without sandbox enforcement
- **THEN** no host process starts and `GET /plugins` reports the plugin host as `failed` with the sandbox error

### Requirement: User configuration
(P1) The system SHALL validate plugin settings under `plugins.<name>.config` against the manifest `userConfig` JSON Schema. It SHALL prompt for required values on install (with masked input for fields marked `secret`), store secrets in the OS keyring, and pass the resolved non-secret config to the host in `initialize`. A secret value SHALL NOT be read from any configuration file; it SHALL reach the host only through `secrets/get`, which requires the `secrets` capability. Invalid settings, or a required value that is not set, SHALL keep the host from starting with an error naming the setting and never its value. `cyber plugins configure <name>` SHALL set settings after install, and removing a plugin SHALL delete its stored secrets. Stored secrets SHALL belong to one plugin in one scope and checkout: a same-named plugin in another scope or checkout SHALL NOT read or delete them. A file-backed secret store SHALL be unreadable to every sandboxed process.

#### Scenario: Secret prompted on install
- **WHEN** a plugin's `userConfig` requires secret `api_token`
- **THEN** install prompts for it with masked input and stores it in the keyring, not in config files

#### Scenario: Same-named project plugin
- **WHEN** a user-scope plugin `jira` has a stored `api_token` and a cloned repository lists a project plugin `jira` that the user approves
- **THEN** the project plugin's `secrets/get` does not return the user plugin's `api_token`

#### Scenario: Missing required secret
- **WHEN** a loaded plugin requires secret `api_token` and none is stored
- **THEN** its host does not start and the plugin reports `plugins.<name>.config.api_token is required`

### Requirement: Capability declaration and enforcement
(P1) A manifest SHALL declare `capabilities` from `fs.read`, `fs.write`, `network` (optionally with host allowlist), `shell`, `secrets` and `session.prompt`. The system SHALL show these before install and SHALL run the plugin host inside the sandbox with only the declared capabilities. An undeclared action SHALL fail inside the plugin with a sandbox denial. A declared `network` SHALL be an upper bound under the resolved sandbox, never a grant over it: with `sandbox.network: "off"` a plugin process SHALL have no network, `sandbox.network.denied_domains` SHALL be refused even for `network: true`, and only a resolved `on` network without a denylist SHALL let `network: true` connect directly. A plugin's hooks SHALL stay within the same capabilities: an `http` hook SHALL load only when `network` covers its URL's host, a `prompt` hook only with `session.prompt`, and an `mcp_tool` hook only when it calls an MCP server that plugin contributes; a hooks file with any other handler SHALL be refused whole with a warning. A plugin's command hooks SHALL run with the plugin's network, not the Session's.

#### Scenario: Undeclared network blocked
- **WHEN** a plugin without `network` tries to open an HTTPS connection
- **THEN** the connection fails with a sandbox denial and a warning naming the plugin

#### Scenario: Declared network under an off sandbox network
- **WHEN** the user's sandbox network is `off` and a plugin declaring `network: true` opens a connection
- **THEN** the connection fails with a sandbox denial naming the plugin

#### Scenario: Network-less plugin hooks
- **WHEN** a plugin without `network` ships an `http` hook and a command hook running `curl`
- **THEN** the `http` hook is not loaded, and the command hook's connection is refused
