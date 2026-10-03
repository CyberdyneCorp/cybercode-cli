# plugins-marketplace Specification

## Purpose
Plugins package reusable extensions (skills, commands, agents, hooks, MCP servers, tools, provider adapters, themes, workflows, channels and TUI mods) into one versioned bundle that can be installed from git, npm, a local path or a marketplace. Because the core is Rust, plugin code runs out-of-process over JSON-RPC 2.0, so a misbehaving plugin cannot crash the agent. This draws on OpenCode v1/v2 (npm/file plugins, scoped replayable registrations, failure isolation), Claude Code (bundles, marketplaces, dependencies, `plugin eval`, mods) and Codex (plugin browsing, trust).

## Requirements

### Requirement: Plugin manifest
(P1) A plugin SHALL be a directory containing `cyber-plugin.json` with required `name` (lowercase, `^[a-z][a-z0-9-]{1,63}$`) and `version` (semver), and optional `description`, `author`, `license`, `homepage`, `engines.cyber` (semver range), `dependencies` (name → semver range), `capabilities`, `userConfig` (JSON Schema) and component paths `skills`, `commands`, `agents`, `hooks`, `mcp`, `tools`, `providers`, `themes`, `workflows`, `channels` and `mods`. A missing or invalid manifest SHALL make the plugin fail to load with an error naming the field.

#### Scenario: Minimal manifest
- **WHEN** a directory contains `cyber-plugin.json` `{ "name": "lint-guard", "version": "1.0.0", "hooks": "hooks.json" }`
- **THEN** the plugin loads and registers the hooks from `hooks.json`

#### Scenario: Invalid name
- **WHEN** the manifest `name` is `Lint Guard`
- **THEN** loading fails with `invalid plugin name`

### Requirement: Component discovery defaults
(P1) When a component path is omitted, the system SHALL look for conventional locations inside the plugin root: `skills/*/SKILL.md`, `commands/**/*.md`, `agents/**/*.md`, `hooks/hooks.json`, `.mcp.json`, `workflows/*.{js,ts}`, `themes/*.json`. Components SHALL be namespaced as `<plugin>:<component>` when they would collide with an existing name.

#### Scenario: Namespaced command on collision
- **WHEN** plugin `review-kit` provides command `review` and a built-in `review` exists
- **THEN** the plugin command is available as `/review-kit:review`

### Requirement: Install scopes
(P1) The system SHALL support install scopes `user` (`~/.config/cyber/plugins.json`), `project` (`.cyber/plugins.json`, committed), `local` (`.cyber/plugins.local.json`, git-ignored) and `managed` (org policy). Plugins SHALL be enabled in the union of scopes, and managed `disabled` entries SHALL override every other scope. Installed files SHALL be cached at `~/.cache/cyber/plugins/<name>/<version>/`.

#### Scenario: Project plugin shared via git
- **WHEN** `.cyber/plugins.json` lists `review-kit@^2.0.0`
- **THEN** a teammate opening the project is prompted once to install and trust `review-kit`

### Requirement: Plugin sources
(P1) The system SHALL install plugins from `git` (URL plus optional `ref`/`path`), `npm` (package spec, installed with lifecycle scripts disabled), `path` (local directory, used in place for development) and `marketplace` (`<plugin>@<marketplace>`). The resolved source and content digest SHALL be recorded in a lockfile `.cyber/plugins.lock` for project scope.

#### Scenario: Lockfile pins digest
- **WHEN** a project plugin is installed from git ref `main`
- **THEN** `.cyber/plugins.lock` records the resolved commit and SHA-256 digest, and later installs use that commit until `cyber plugins update`

### Requirement: Plugin CLI
(P1) The system SHALL provide `cyber plugins install <spec> [--scope user|project|local]`, `list`, `info <name>`, `update [name]`, `remove <name>`, `enable <name>`, `disable <name>` and `validate <path>`, plus the in-session `/plugins` browser and `/reload-plugins`. `install` SHALL refuse a version whose `engines.cyber` range excludes the running version, unless `--force` is given.

#### Scenario: Engine mismatch
- **WHEN** a plugin declares `engines.cyber: ">=2.0.0"` and the running version is `1.4.0`
- **THEN** `cyber plugins install` fails with `requires cyber >=2.0.0, running 1.4.0`

### Requirement: Dependencies
(P4) The system SHALL resolve `dependencies` by semver range across all enabled plugins, install missing dependencies in the same scope, fail with a conflict report when no version satisfies all ranges, and prune dependencies no longer required by any plugin on `update` or `remove`.

#### Scenario: Conflicting ranges
- **WHEN** plugin A requires `utils@^1` and plugin B requires `utils@^2`
- **THEN** installation of the second plugin fails listing both requirers and ranges

### Requirement: Plugin host process
(P1) Plugins that declare `tools`, `providers`, `hooks.runtime` or `channels` code SHALL run in a dedicated host process per plugin, launched from the manifest `main` with the declared `runtime` (`node`, `bun`, `deno`, `python` or a native executable). The host SHALL communicate over JSON-RPC 2.0 on stdio. The core SHALL never load plugin code in-process.

#### Scenario: Plugin crash isolated
- **WHEN** a plugin host process exits unexpectedly during a tool call
- **THEN** that call fails with `plugin <name> crashed` and the session continues

### Requirement: Host protocol
(P1) The plugin protocol SHALL include `initialize` (capability negotiation: protocol version, contributed tools, hooks, providers, catalog transforms), `tool/execute`, `hook/<event>`, `provider/stream`, `catalog/transform`, `config/changed`, `shutdown`, and host-to-core requests `session/prompt`, `permission/ask`, `log` and `secrets/get`. Protocol version mismatches SHALL fail `initialize` with both versions reported.

#### Scenario: Tool contributed over JSON-RPC
- **WHEN** a plugin's `initialize` result declares tool `jira_search` with an input schema
- **THEN** the tool appears in the registry as `jira_search` and calls are forwarded via `tool/execute`

### Requirement: Restart and backoff
(P1) The system SHALL restart a crashed plugin host with exponential backoff starting at 1 s and capped at 60 s. After 5 crashes within 10 minutes, the system SHALL disable the plugin for the process lifetime and report `plugin <name> disabled after repeated crashes`.

#### Scenario: Crash loop disables plugin
- **WHEN** a plugin host crashes 5 times within 10 minutes
- **THEN** the plugin is disabled and its tools disappear from the next Turn's tool list

### Requirement: Scoped registrations
(P1) Every registration a plugin makes (tools, hooks, commands, agents, catalog transforms, providers) SHALL be owned by that plugin's scope. Disabling, removing, updating or crashing the plugin SHALL remove exactly its registrations and trigger a rebuild of affected domains, without restarting the server.

#### Scenario: Disable removes contributions
- **WHEN** the user runs `cyber plugins disable review-kit`
- **THEN** its commands, agents and hooks disappear without a restart and other plugins are unaffected

### Requirement: Capability declaration and enforcement
(P1) A manifest SHALL declare `capabilities` from `fs.read`, `fs.write`, `network` (optionally with host allowlist), `shell`, `secrets` and `session.prompt`. The system SHALL show these before install and SHALL run the plugin host inside the sandbox with only the declared capabilities. An undeclared action SHALL fail inside the plugin with a sandbox denial.

#### Scenario: Undeclared network blocked
- **WHEN** a plugin without `network` tries to open an HTTPS connection
- **THEN** the connection fails with a sandbox denial and a warning naming the plugin

### Requirement: User configuration
(P1) The system SHALL validate plugin settings under `plugins.<name>.config` against the manifest `userConfig` JSON Schema. It SHALL prompt for required values on install (with masked input for fields marked `secret`), store secrets in the OS keyring, and pass resolved config to the host in `initialize`.

#### Scenario: Secret prompted on install
- **WHEN** a plugin's `userConfig` requires secret `api_token`
- **THEN** install prompts for it with masked input and stores it in the keyring, not in config files

### Requirement: TypeScript plugin kit
(P1) The system SHALL publish `@cyber-code/plugin` providing `definePlugin`, `tool({ description, input, output, execute })` with Zod or JSON Schema, typed hook handlers, a provider adapter helper and a test harness that drives the JSON-RPC protocol in-memory.

#### Scenario: TS plugin tool
- **WHEN** a developer exports `definePlugin({ tools: { hello: tool({...}) } })` and runs `cyber plugins install ./my-plugin --scope local`
- **THEN** the tool `hello` is callable by the model

### Requirement: Pure mode
(P1) The global `--pure` flag and `CYBER_PURE=1` SHALL disable all non-built-in plugins, project hooks and project MCP servers for that process while keeping built-ins.

#### Scenario: Pure mode run
- **WHEN** the user runs `cyber --pure`
- **THEN** no external plugin host is spawned and `/plugins` shows them as disabled by `--pure`

### Requirement: Marketplaces
(P4) The system SHALL support marketplaces defined by a `marketplace.json` index (`name`, `owner`, `plugins: [{ name, source, version, description, tags, relevance? }]`) hosted in git or over HTTPS. Users add them with `cyber plugins marketplace add <source>` and refresh them every 24 h. The official marketplace `cyber-official` SHALL be preconfigured and removable.

#### Scenario: Install from marketplace
- **WHEN** the user runs `cyber plugins install pr-tools@cyber-official`
- **THEN** the plugin resolves from that marketplace index and installs its listed source

### Requirement: Organization plugin controls
(P4) Org policy SHALL be able to allowlist marketplaces and plugins, force-install plugins (`managed` scope), and forbid installs from unlisted sources. Blocked installs SHALL fail with `blocked by organization policy`.

#### Scenario: Unlisted marketplace blocked
- **WHEN** policy allows only `cyber-official` and the user adds another marketplace
- **THEN** the command fails with `blocked by organization policy`

### Requirement: Trust and signing
(P1) The system SHALL record trust per plugin and version digest using workspace-trust. It SHALL prompt before first enabling a plugin from project scope, and in P4 SHALL verify Sigstore signatures when the marketplace entry declares `signature`, refusing install on mismatch. Unsigned plugins SHALL be marked `unsigned` in listings.

#### Scenario: Signature mismatch
- **WHEN** a marketplace entry declares a signature that does not verify against the downloaded archive
- **THEN** install is refused with `signature verification failed`

### Requirement: Plugin evals
(P4) The system SHALL provide `cyber plugins eval <path>`, which runs eval cases from `evals/*.yaml` (prompt, fixture repo, assertions, optional LLM grader rubric) with and without the plugin. It SHALL report pass rate, cost and token delta versus the baseline, write `eval-report.json`, and exit non-zero when the score is below `--min-score` (default 0.8).

#### Scenario: CI gate on eval score
- **WHEN** `cyber plugins eval . --min-score 0.9` scores 0.85
- **THEN** the command exits with code 1 and prints the failing cases

### Requirement: Plugin usage telemetry
(P4) The system SHALL attribute tokens and cost spent in plugin-contributed tools, hooks (prompt type), skills and agents to the plugin name in usage records, and SHALL expose a per-plugin rollup in `cyber stats --plugins`.

#### Scenario: Plugin cost rollup
- **WHEN** the user runs `cyber stats --plugins --days 7`
- **THEN** each plugin is listed with invocations, tokens and cost for the last 7 days

### Requirement: Mods (TUI extensions)
(P4) A plugin MAY declare `mods` that run in the plugin host. Mods MAY add TUI panes, a band above the prompt, buttons, text fields, commands and tool-call rules, through a render protocol of declarative elements (`text`, `markdown`, `code`, `diff`, `button`, `field`, `list`). The TUI SHALL redraw mods on state change, limit each mod to 30 redraws per second, and allow disabling mods per plugin. Org policy SHALL be able to forbid user-installed mods.

#### Scenario: Mod adds a pane
- **WHEN** an enabled mod renders a pane with a `list` of failing tests and a `button` "rerun"
- **THEN** the TUI shows the pane and pressing the button sends `mod/press` to the plugin host

### Requirement: Plugin-contributed workflows and channels
(P2) Workflows under the plugin's `workflows/` directory SHALL be registered as saved workflows named `<plugin>:<workflow>`, and (P3) channel adapters declared in `channels` SHALL be registered as channel types, both subject to the same scope ownership rules.

#### Scenario: Plugin workflow available
- **WHEN** plugin `audit-kit` ships `workflows/security-sweep.ts`
- **THEN** `/workflows` lists `audit-kit:security-sweep` as runnable
