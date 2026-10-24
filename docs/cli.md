# Command-line reference

<!-- Generated from the clap definition by `cyber help-markdown`. Do not edit; regenerate with `just docs-cli`. -->

Generated from `cyber`'s argument definitions; `cyber <command> --help` prints the same text. Options under `cyber` marked Global apply to every subcommand and are not repeated below.

## `cyber`

Cyber Code: a model-independent coding agent you can interrupt, inspect and resume

```text
cyber [OPTIONS] [COMMAND]
```

**Options:**

- `-V, --version`: Print version information.
- `--cwd <PATH>`: Working directory (the Location). The only working-directory flag. Global.
- `--profile <NAME>`: Named profile from the `profiles` config key. Global.
- `--config <KEY=VALUE>`: Set one config value for this process, as a dotted key and a JSON or string value. Global.
- `-m, --model <MODEL>`: Model as provider/model[#variant]. Global.
- `--mode <MODE>`: Permission mode: default, accept-edits, plan, auto, dont-ask or bypass. Global.
- `--log-level <LEVEL>`: Minimum log level: error, warn, info (default), debug or trace. Also CYBER_LOG_LEVEL. Global.
- `--print-logs`: Mirror log lines to stderr. Global.
- `--format <FORMAT>`: Output format. The only output-format flag. Values: `table`, `json`, `text`, `stream-json`, `md`, `html`. Global.
- `--pure`: Disable plugins and every hook and MCP server not managed by organization policy for this process. Also CYBER_PURE=1. Global.
- `--features <LIST>`: Turn features on or off for this process, as a comma list where `-name` turns one off (`teams,-other`). Above CYBER_FEATURES and config; below organization policy. Commands served by the background server (`cyber teams list`, the TUI) follow its configuration instead: use `cyber features enable`, or the TUI with `--embedded`. Global.
- `--worktree [<NAME>]`: Start a new Session in a managed worktree; omit NAME to generate one.
- `-c, --continue`: Resume the most recent Session of the Location.
- `-r, --resume [<RESUME>]`: Resume a Session by ID or name; without a value, open the picker.
- `--fork`: Copy the selected Session into a new one.
- `--agent <AGENT>`
- `--prompt <PROMPT>`: Submit an initial prompt (piped stdin is prepended)
- `--embedded`: Run a private in-process server instead of the background service.
- `--sandbox <SANDBOX>`: Sandbox policy for an embedded server: read-only, workspace-write or full-access.
- `--dangerously-bypass-permissions`: Allow `bypass` mode in an embedded server without a sandbox or a container.
- `--dangerously-skip-permissions`: Claude Code's name for running without prompts: start in `bypass` mode with --dangerously-bypass-permissions, so every action runs without approval and, outside a sandbox or a container, without OS confinement. Needs --embedded: the shared server cannot take the override for one TUI (there, use --mode bypass where its sandbox or a container already admits it). An organization policy that disables `bypass` still refuses it.
- `--from-pr <NUMBER|URL>`: Open the Session picker over the Sessions linked to this pull request.

**Subcommands:**

- [`import`](#cyber-import): Import Claude Code, Codex or OpenCode configuration: detect, preview or write
- [`lsp`](#cyber-lsp): Inspect installed language servers for the current Location
- [`fmt`](#cyber-fmt): Inspect detected formatters for the current Location
- [`memory`](#cyber-memory): Review and edit durable project/global memory
- [`sessions`](#cyber-sessions): List, show, rename and delete the Location's Sessions
- [`skills`](#cyber-skills): Inspect skills: source, scope, permission effect and diagnostics
- [`commands`](#cyber-commands): Inspect slash commands and command discovery diagnostics
- [`mcp`](#cyber-mcp): Review MCP server definitions, individual server approvals and OAuth credentials
- [`teams`](#cyber-teams): Agent teams (feature `teams`): list, show, stop and resume teams; list definitions
- [`features`](#cyber-features): List the feature registry; enable or disable a feature in config
- [`workflows`](#cyber-workflows): List saved Workflows; run, inspect and control Workflow Runs
- [`worktree`](#cyber-worktree): List, remove and prune managed Git worktrees of the current repository
- [`debug`](#cyber-debug): Inspect resolved paths, configuration and build information
- [`models`](#cyber-models): List models from the catalog, available first
- [`providers`](#cyber-providers): Sign in to or out of a provider (today: `chatgpt`, a ChatGPT Plus/Pro subscription, and `copilot`, a GitHub Copilot plan)
- [`permissions`](#cyber-permissions): Dry-run permission rules; inspect or reset checkout-scoped auto-mode statistics
- [`db`](#cyber-db): Inspect the local database
- [`plugins`](#cyber-plugins): Install, list, enable, disable, update and remove plugins
- [`hooks`](#cyber-hooks): Review hook definitions and individual handler trust
- [`sandbox`](#cyber-sandbox): Test, explain or run a command under the effective OS sandbox policy
- [`trust`](#cyber-trust): Inspect and approve repository-controlled configuration for this checkout
- [`exec`](#cyber-exec): Run one prompt non-interactively (alias: cyber -p)
- [`attach`](#cyber-attach): Open the TUI on a Session of a paired machine: cyber attach <session>@<peer>
- [`review`](#cyber-review): Review uncommitted changes, a range, a commit or a pull request
- [`pr`](#cyber-pr): Check out a pull request and resume the Session linked to it
- [`git`](#cyber-git): Generate commit and pull request text with the small model
- [`serve`](#cyber-serve): Run the server in the foreground
- [`service`](#cyber-service): Manage the background server
- [`api`](#cyber-api): Send one request to the server API
- [`peers`](#cyber-peers): Pair with other machines' cyber servers to message their Sessions
- [`messages`](#cyber-messages): Send messages between Sessions and read their message history
- [`doctor`](#cyber-doctor): Check configuration, skills, commands, credentials, catalog, database, sandbox, tools and server
- [`stats`](#cyber-stats): Usage statistics: activity, streaks, tokens, cost and per-model totals
- [`upgrade`](#cyber-upgrade): Upgrade cyber to the channel's latest version or to VERSION
- [`uninstall`](#cyber-uninstall): Remove cyber: its binary (script installs), PATH lines and directories
- [`eval`](#cyber-eval): Run evaluation manifests and write reports

## `cyber import`

Import Claude Code, Codex or OpenCode configuration: detect, preview or write

```text
cyber import [OPTIONS] [TOOL]
```

**Arguments:**

- `[TOOL]`: Values: `claude`, `codex`, `opencode`, `auto`.

**Options:**

- `--detect`
- `--dry-run`: Print the file diff and the mapping report; never write.
- `--yes`: Write without asking for confirmation.
- `--include <INCLUDE>`: Also import these items. Values: `sessions`.
- `--scope <SCOPE>`: Values: `project`, `global`. Default: `project`.

## `cyber lsp`

Inspect installed language servers for the current Location

```text
cyber lsp [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`status`](#cyber-lsp-status)

## `cyber lsp status`

```text
cyber lsp status [OPTIONS]
```

## `cyber fmt`

Inspect detected formatters for the current Location

```text
cyber fmt [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`status`](#cyber-fmt-status)

## `cyber fmt status`

```text
cyber fmt status [OPTIONS]
```

## `cyber memory`

Review and edit durable project/global memory

```text
cyber memory [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-memory-list): List note metadata and invalid-file diagnostics
- [`show`](#cyber-memory-show): Print a memory's content
- [`edit`](#cyber-memory-edit): Edit a private draft with EDITOR (or VISUAL), then validate and commit it
- [`delete`](#cyber-memory-delete): Delete a note and update its index through the recovery journal
- [`recovery`](#cyber-memory-recovery): Inspect an interrupted transaction and its proposed content without recovery
- [`recover`](#cyber-memory-recover): Recover local files only if the reviewed fingerprint still matches
- [`recovery-request`](#cyber-memory-recovery-request): Inspect a retained server recovery request without repeating its effects
- [`path`](#cyber-memory-path): Print the memory directory without creating it

## `cyber memory list`

List note metadata and invalid-file diagnostics

```text
cyber memory list [OPTIONS]
```

## `cyber memory show`

Print a memory's content

```text
cyber memory show [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber memory edit`

Edit a private draft with EDITOR (or VISUAL), then validate and commit it

```text
cyber memory edit [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber memory delete`

Delete a note and update its index through the recovery journal

```text
cyber memory delete [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber memory recovery`

Inspect an interrupted transaction and its proposed content without recovery

```text
cyber memory recovery [OPTIONS]
```

## `cyber memory recover`

Recover local files only if the reviewed fingerprint still matches

```text
cyber memory recover [OPTIONS] --review <REVIEW>
```

**Options:**

- `--review <REVIEW>`
- `--admission-review <ADMISSION_REVIEW>`: Database fingerprint from recovery --server.
- `--key <KEY>`: Retain this key for retries and durable receipt lookup.

## `cyber memory recovery-request`

Inspect a retained server recovery request without repeating its effects

```text
cyber memory recovery-request [OPTIONS] --server --key <KEY>
```

**Options:**

- `--key <KEY>`

## `cyber memory path`

Print the memory directory without creating it

```text
cyber memory path [OPTIONS]
```

## `cyber sessions`

List, show, rename and delete the Location's Sessions

```text
cyber sessions [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-sessions-list): The Location's Sessions, most recently updated first
- [`show`](#cyber-sessions-show): One Session: title, Location, model, status and usage
- [`rename`](#cyber-sessions-rename): Give a Session a title to find and resume it by later
- [`delete`](#cyber-sessions-delete): Delete a Session with its child Sessions
- [`tree`](#cyber-sessions-tree): The Session's user-message tree: `*` marks the active path
- [`checkout`](#cyber-sessions-checkout): Make another branch active; the Session must be idle
- [`label`](#cyber-sessions-label): Set or remove a branch label
- [`export`](#cyber-sessions-export): Write a Session as JSON (default), Markdown (--format md) or one self-contained HTML page (--format html): its info, messages, tool calls, goals and workflow runs

## `cyber sessions list`

The Location's Sessions, most recently updated first

```text
cyber sessions list [OPTIONS]
```

**Options:**

- `--limit <LIMIT>`: At most this many (1 to 200) Default: `50`.

## `cyber sessions show`

One Session: title, Location, model, status and usage

```text
cyber sessions show [OPTIONS] <SESSION>
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.

## `cyber sessions rename`

Give a Session a title to find and resume it by later

```text
cyber sessions rename [OPTIONS] <SESSION> <TITLE>...
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.
- `<TITLE>...`: The new title; words are joined with spaces.

## `cyber sessions delete`

Delete a Session with its child Sessions

```text
cyber sessions delete [OPTIONS] <SESSION>
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.

**Options:**

- `--yes`: Confirm the deletion; it cannot be undone.

## `cyber sessions tree`

The Session's user-message tree: `*` marks the active path

```text
cyber sessions tree [OPTIONS] <SESSION>
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.

## `cyber sessions checkout`

Make another branch active; the Session must be idle

```text
cyber sessions checkout [OPTIONS] <SESSION> <MESSAGE>
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.
- `<MESSAGE>`: A message ID, a branch label or a unique message ID prefix; the newest tip under it becomes active.

**Options:**

- `--code`: Also restore files to the branch's last snapshot, keeping your own edits.
- `--exact`: Make the message itself the tip; the next prompt branches there.

## `cyber sessions label`

Set or remove a branch label

```text
cyber sessions label [OPTIONS] <SESSION> <NAME>
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.
- `<NAME>`: The label: letters, digits, `.`, `_` and `-`, at most 48.

**Options:**

- `--at <AT>`: The message to label (ID, label or unique prefix); the active tip by default.
- `--delete`: Remove the label instead.

## `cyber sessions export`

Write a Session as JSON (default), Markdown (--format md) or one self-contained HTML page (--format html): its info, messages, tool calls, goals and workflow runs

```text
cyber sessions export [OPTIONS] <SESSION>
```

**Arguments:**

- `<SESSION>`: ID, exact title or unique ID prefix.

**Options:**

- `--sanitize`: Replace prompts, answers, paths, URLs, tool inputs and outputs and metadata with [redacted:<kind>:<n>] markers, keeping structure, token counts and timings.
- `--include-children`: Include child Sessions (subagents, workflow agents), recursively.
- `--all-branches`: Include every branch; each message's `parent_message_id` links the tree.
- `-o, --output <FILE>`: Write the export to this file, atomically, instead of stdout. A symlink is refused; an existing file keeps its permissions, a new one is private.

## `cyber skills`

Inspect skills: source, scope, permission effect and diagnostics

```text
cyber skills [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-skills-list): List skills with source, scope, permission effect and diagnostics
- [`show`](#cyber-skills-show): Show one skill, including skipped definitions of that name
- [`trust`](#cyber-skills-trust): Approve a remote skill source (`https://` entry) configured outside your user config for this checkout, bound to its URL and currently cached content

## `cyber skills list`

List skills with source, scope, permission effect and diagnostics

```text
cyber skills list [OPTIONS]
```

**Options:**

- `--agent <AGENT>`: Evaluate permissions for this agent. Default: `build`.

## `cyber skills show`

Show one skill, including skipped definitions of that name

```text
cyber skills show [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--agent <AGENT>`: Evaluate permissions for this agent. Default: `build`.

## `cyber skills trust`

Approve a remote skill source (`https://` entry) configured outside your user config for this checkout, bound to its URL and currently cached content

```text
cyber skills trust [OPTIONS] <URL>
```

**Arguments:**

- `<URL>`

## `cyber commands`

Inspect slash commands and command discovery diagnostics

```text
cyber commands [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-commands-list): List slash commands and command discovery diagnostics
- [`trust`](#cyber-commands-trust): Approve a project command or skill template that runs shell commands (`` !`cmd` ``) for this checkout. A changed template needs a new approval

## `cyber commands list`

List slash commands and command discovery diagnostics

```text
cyber commands list [OPTIONS]
```

## `cyber commands trust`

Approve a project command or skill template that runs shell commands (`` !`cmd` ``) for this checkout. A changed template needs a new approval

```text
cyber commands trust [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber mcp`

Review MCP server definitions, individual server approvals and OAuth credentials

```text
cyber mcp [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-mcp-list): List configured servers with their live status, transport and tool count
- [`add`](#cyber-mcp-add): Add a server to the user, project or local config document, keeping its comments: `--url <url>` for a remote server, or `-- <command> [args...]` for a local one
- [`remove`](#cyber-mcp-remove): Remove a server from the config document that defines it
- [`definitions`](#cyber-mcp-definitions): Review loaded server definitions, origins and digests without connecting
- [`get`](#cyber-mcp-get): Display one loaded server and its resolved output options without connecting
- [`trust`](#cyber-mcp-trust): Approve an inspected, currently loaded project server definition
- [`untrust`](#cyber-mcp-untrust): Revoke an individual server digest, including an obsolete definition
- [`auth`](#cyber-mcp-auth): Authorize a remote server with OAuth: prints the authorization URL to open in a browser, waits for the loopback callback and stores the token privately
- [`serve`](#cyber-mcp-serve): Serve Cyber Code Sessions to an MCP client: a stdio bridge to the shared server, or `--transport http` to print its endpoint. Reads only the global config and never opens the database
- [`logout`](#cyber-mcp-logout): Remove every stored OAuth credential of a server (at any URL, from any issuer) and ask a running server to reconnect without it. Tokens are not revoked remotely
- [`browser`](#cyber-mcp-browser): Install, check and clean up the optional browser verification server

## `cyber mcp list`

List configured servers with their live status, transport and tool count

```text
cyber mcp list [OPTIONS]
```

## `cyber mcp add`

Add a server to the user, project or local config document, keeping its comments: `--url <url>` for a remote server, or `-- <command> [args...]` for a local one

```text
cyber mcp add [OPTIONS] <NAME> [-- <COMMAND>...]
```

**Arguments:**

- `<NAME>`: Server name (letters, digits, `-` and `_`)
- `[COMMAND]...`: A local (stdio) server: the command and its arguments, after `--`.

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`. Default: `user`.
- `--url <URL>`: A remote (Streamable HTTP) server URL.
- `--header <K=V>`: A request header for a remote server, as `Name=value` (repeatable)
- `--env <K=V>`: An environment variable for a local server, as `NAME=value` (repeatable)

## `cyber mcp remove`

Remove a server from the config document that defines it

```text
cyber mcp remove [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`.

## `cyber mcp definitions`

Review loaded server definitions, origins and digests without connecting

```text
cyber mcp definitions [OPTIONS]
```

## `cyber mcp get`

Display one loaded server and its resolved output options without connecting

```text
cyber mcp get [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber mcp trust`

Approve an inspected, currently loaded project server definition

```text
cyber mcp trust [OPTIONS] --digest <DIGEST> <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--digest <DIGEST>`

## `cyber mcp untrust`

Revoke an individual server digest, including an obsolete definition

```text
cyber mcp untrust [OPTIONS] --digest <DIGEST>
```

**Options:**

- `--digest <DIGEST>`

## `cyber mcp auth`

Authorize a remote server with OAuth: prints the authorization URL to open in a browser, waits for the loopback callback and stores the token privately

```text
cyber mcp auth [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--port <PORT>`: Loopback callback port (overrides `oauth.redirect_port`; default ephemeral)

## `cyber mcp serve`

Serve Cyber Code Sessions to an MCP client: a stdio bridge to the shared server, or `--transport http` to print its endpoint. Reads only the global config and never opens the database

```text
cyber mcp serve [OPTIONS]
```

**Options:**

- `--transport <TRANSPORT>`: Values: `stdio`, `http`. Default: `stdio`.
- `--client <LABEL>`: Attribution label sent as X-Cyber-MCP-Client (^[a-z0-9][a-z0-9_-]{0,47}$); defaults to the client's `clientInfo.name`.

## `cyber mcp logout`

Remove every stored OAuth credential of a server (at any URL, from any issuer) and ask a running server to reconnect without it. Tokens are not revoked remotely

```text
cyber mcp logout [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber mcp browser`

Install, check and clean up the optional browser verification server

```text
cyber mcp browser [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`install`](#cyber-mcp-browser-install): Write the pinned browser MCP server and enable browser verification
- [`health`](#cyber-mcp-browser-health): Start the configured browser server once and check the tools it offers
- [`cleanup`](#cyber-mcp-browser-cleanup): Remove per-Session browser profiles that no running process holds

## `cyber mcp browser install`

Write the pinned browser MCP server and enable browser verification

```text
cyber mcp browser install [OPTIONS]
```

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`. Default: `user`.
- `--server <SERVER>`: The MCP server name (letters, digits, `-` and `_`) Default: `browser`.

## `cyber mcp browser health`

Start the configured browser server once and check the tools it offers

```text
cyber mcp browser health [OPTIONS]
```

## `cyber mcp browser cleanup`

Remove per-Session browser profiles that no running process holds

```text
cyber mcp browser cleanup [OPTIONS]
```

## `cyber teams`

Agent teams (feature `teams`): list, show, stop and resume teams; list definitions

```text
cyber teams [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-teams-list): The Location's teams, newest first
- [`show`](#cyber-teams-show): A team's members (status, current task, cost, last message) and its task board
- [`stop`](#cyber-teams-stop): Stop a team: teammates finish their current Turn; queued input stays queued
- [`resume`](#cyber-teams-resume): Resume a stopped team; after its lead was deleted, `--lead` names the new one
- [`definitions`](#cyber-teams-definitions): Team definitions from `.cyber/teams/*.md` (trusted checkouts) and `~/.config/cyber/teams/*.md`; a project definition wins on a name clash

## `cyber teams list`

The Location's teams, newest first

```text
cyber teams list [OPTIONS]
```

**Options:**

- `--status <STATUS>`: Only teams with this status. Values: `active`, `stopping`, `stopped`.

## `cyber teams show`

A team's members (status, current task, cost, last message) and its task board

```text
cyber teams show [OPTIONS] <TEAM>
```

**Arguments:**

- `<TEAM>`: A `team_` ID, or the name of a team of this Location (the newest one by that name)

## `cyber teams stop`

Stop a team: teammates finish their current Turn; queued input stays queued

```text
cyber teams stop [OPTIONS] <TEAM>
```

**Arguments:**

- `<TEAM>`: A `team_` ID, or the name of a team of this Location (the newest one by that name)

## `cyber teams resume`

Resume a stopped team; after its lead was deleted, `--lead` names the new one

```text
cyber teams resume [OPTIONS] <TEAM>
```

**Arguments:**

- `<TEAM>`: A `team_` ID, or the name of a team of this Location (the newest one by that name)

**Options:**

- `--lead <LEAD>`: The new lead (ID, exact title or unique ID prefix) of a team whose lead was deleted.

## `cyber teams definitions`

Team definitions from `.cyber/teams/*.md` (trusted checkouts) and `~/.config/cyber/teams/*.md`; a project definition wins on a name clash

```text
cyber teams definitions [OPTIONS]
```

## `cyber features`

List the feature registry; enable or disable a feature in config

```text
cyber features [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-features-list): Every feature with its maturity, default, effective value and source
- [`enable`](#cyber-features-enable): Turn a feature on in the user or project configuration
- [`disable`](#cyber-features-disable): Turn a feature off in the user or project configuration

## `cyber features list`

Every feature with its maturity, default, effective value and source

```text
cyber features list [OPTIONS]
```

**Options:**

- `--json`: Print JSON (same as --format json)

## `cyber features enable`

Turn a feature on in the user or project configuration

```text
cyber features enable [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`. Default: `user`.

## `cyber features disable`

Turn a feature off in the user or project configuration

```text
cyber features disable [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`. Default: `user`.

## `cyber workflows`

List saved Workflows; run, inspect and control Workflow Runs

```text
cyber workflows [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-workflows-list): List the Workflow library (project overrides global, both override bundled) and modules that fail to load
- [`eject`](#cyber-workflows-eject): Copy a bundled Workflow's source into `.cyber/workflows/` (or the global directory) to customize it; the copy then overrides the bundled one
- [`run`](#cyber-workflows-run): Start a Workflow Run from a saved name or a script file
- [`runs`](#cyber-workflows-runs): List recent Workflow Runs of this Location: active runs first, then the newest
- [`show`](#cyber-workflows-show): Show one Workflow Run and its agents (`run_…`), or a library Workflow's meta, args schema, default budget and origin (a Workflow name)
- [`pause`](#cyber-workflows-pause): Pause a Workflow Run: in-flight agents finish, no new agent starts
- [`resume`](#cyber-workflows-resume): Resume a paused, failed or stopped Workflow Run, or with `--interrupted` every run a server restart interrupted
- [`stop`](#cyber-workflows-stop): Stop a Workflow Run and cancel its in-flight agents
- [`logs`](#cyber-workflows-logs): Print a Workflow Run's log

## `cyber workflows list`

List the Workflow library (project overrides global, both override bundled) and modules that fail to load

```text
cyber workflows list [OPTIONS]
```

## `cyber workflows eject`

Copy a bundled Workflow's source into `.cyber/workflows/` (or the global directory) to customize it; the copy then overrides the bundled one

```text
cyber workflows eject [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`: The bundled Workflow to copy.

**Options:**

- `--global`: Write it to the global `workflows/` directory instead of the project.
- `--force`: Replace an existing file.

## `cyber workflows run`

Start a Workflow Run from a saved name or a script file

```text
cyber workflows run [OPTIONS] <TARGET>
```

**Arguments:**

- `<TARGET>`: A saved Workflow's name, or the path of a `.js` module.

**Options:**

- `--args <JSON>`: The script's `args` as a JSON object.
- `--max-cost <USD>`: Stop starting agents once the run cost this much (USD)
- `--max-turns <N>`
- `--max-tokens <N>`
- `--timeout <DURATION>`: Wall-clock limit of the run, e.g. 30m or 90s.
- `--max-agents <N>`
- `--max-concurrent <N>`: The run's share of the server's Workflow agent slots (at most N agents at once)
- `--runner <ID>`: Run on a remote runner (not available yet)
- `--wait`: Follow the run until it ends; the exit code reports how it ended.

## `cyber workflows runs`

List recent Workflow Runs of this Location: active runs first, then the newest

```text
cyber workflows runs [OPTIONS]
```

**Options:**

- `--all`: Every Location's runs.
- `--status <S[,S...]>`: Only runs of these statuses, e.g. `running,queued`.
- `--watch`: Keep the list current until Ctrl-C, which changes no run.

## `cyber workflows show`

Show one Workflow Run and its agents (`run_…`), or a library Workflow's meta, args schema, default budget and origin (a Workflow name)

```text
cyber workflows show [OPTIONS] <TARGET>
```

**Arguments:**

- `<TARGET>`: A run ID, or a Workflow name.

**Options:**

- `--source`: With a Workflow name, also print the module source.

## `cyber workflows pause`

Pause a Workflow Run: in-flight agents finish, no new agent starts

```text
cyber workflows pause [OPTIONS] <RUN_ID>
```

**Arguments:**

- `<RUN_ID>`

## `cyber workflows resume`

Resume a paused, failed or stopped Workflow Run, or with `--interrupted` every run a server restart interrupted

```text
cyber workflows resume [OPTIONS] [RUN_ID]
```

**Arguments:**

- `[RUN_ID]`

**Options:**

- `--revision`: Accept a changed script or configuration as a new revision.
- `--interrupted`: Resume every run of this Location that a server restart interrupted; runs paused by a user stay paused.
- `--all`: With `--interrupted`, every Location's interrupted runs.

## `cyber workflows stop`

Stop a Workflow Run and cancel its in-flight agents

```text
cyber workflows stop [OPTIONS] <RUN_ID>
```

**Arguments:**

- `<RUN_ID>`

## `cyber workflows logs`

Print a Workflow Run's log

```text
cyber workflows logs [OPTIONS] <RUN_ID>
```

**Arguments:**

- `<RUN_ID>`

## `cyber worktree`

List, remove and prune managed Git worktrees of the current repository

```text
cyber worktree [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-worktree-list): List owned worktrees, Git status and their Sessions
- [`remove`](#cyber-worktree-remove): Remove a managed worktree and its branch
- [`prune`](#cyber-worktree-prune): Remove clean managed worktrees older than a number of days

## `cyber worktree list`

List owned worktrees, Git status and their Sessions

```text
cyber worktree list [OPTIONS]
```

## `cyber worktree remove`

Remove a managed worktree and its branch

```text
cyber worktree remove [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--force`: Remove even with uncommitted changes or commits beyond its base.

## `cyber worktree prune`

Remove clean managed worktrees older than a number of days

```text
cyber worktree prune [OPTIONS]
```

**Options:**

- `--older-than <DAYS>`: Default: `14`.

## `cyber debug`

Inspect resolved paths, configuration and build information

```text
cyber debug [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`memory`](#cyber-debug-memory): List memory metadata and invalid-file diagnostics
- [`paths`](#cyber-debug-paths): Print resolved directories and the database location
- [`config`](#cyber-debug-config): Print the resolved configuration with secrets redacted
- [`info`](#cyber-debug-info): Print build, path, database, configuration and trust information

## `cyber debug memory`

List memory metadata and invalid-file diagnostics

```text
cyber debug memory [OPTIONS]
```

**Options:**

- `--global`

## `cyber debug paths`

Print resolved directories and the database location

```text
cyber debug paths [OPTIONS]
```

## `cyber debug config`

Print the resolved configuration with secrets redacted

```text
cyber debug config [OPTIONS]
```

**Options:**

- `--sources`: Include the source layer of every value.

## `cyber debug info`

Print build, path, database, configuration and trust information

```text
cyber debug info [OPTIONS]
```

## `cyber models`

List models from the catalog, available first

```text
cyber models [OPTIONS] [PROVIDER]
```

**Arguments:**

- `[PROVIDER]`: Only this provider (`list` and `refresh` are accepted as verbs)

**Options:**

- `--verbose`: Show limits, prices and variants.
- `--refresh`: Refetch the catalog even when the cache is fresh.

## `cyber providers`

Sign in to or out of a provider (today: `chatgpt`, a ChatGPT Plus/Pro subscription, and `copilot`, a GitHub Copilot plan)

```text
cyber providers [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`login`](#cyber-providers-login): Sign in to a provider. `chatgpt` signs in with a ChatGPT Plus/Pro subscription: open the printed URL in a browser to authorize. `copilot` signs in with a GitHub Copilot plan: open the printed URL and enter the printed code
- [`logout`](#cyber-providers-logout): Forget a provider login. Tokens are not revoked remotely

## `cyber providers login`

Sign in to a provider. `chatgpt` signs in with a ChatGPT Plus/Pro subscription: open the printed URL in a browser to authorize. `copilot` signs in with a GitHub Copilot plan: open the printed URL and enter the printed code

```text
cyber providers login [OPTIONS] <PROVIDER>
```

**Arguments:**

- `<PROVIDER>`

**Options:**

- `--method <METHOD>`: `browser` waits for the redirect on localhost:1455; `code` reads the redirect URL pasted from the browser (for SSH and other headless machines). ChatGPT only. Values: `browser`, `code`. Default: `browser`.
- `--enterprise <HOST>`: Sign in to a GitHub Enterprise host (a bare hostname such as example.ghe.com) instead of github.com. Copilot only.

## `cyber providers logout`

Forget a provider login. Tokens are not revoked remotely

```text
cyber providers logout [OPTIONS] <PROVIDER>
```

**Arguments:**

- `<PROVIDER>`

## `cyber permissions`

Dry-run permission rules; inspect or reset checkout-scoped auto-mode statistics

```text
cyber permissions [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`auto`](#cyber-permissions-auto): Show or reset recorded auto-mode decision statistics for this checkout
- [`test`](#cyber-permissions-test): Evaluate a tool call or shell command as a Session would, without running it

## `cyber permissions auto`

Show or reset recorded auto-mode decision statistics for this checkout

```text
cyber permissions auto [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`show`](#cyber-permissions-auto-show)
- [`reset`](#cyber-permissions-auto-reset)

## `cyber permissions auto show`

```text
cyber permissions auto show [OPTIONS]
```

## `cyber permissions auto reset`

```text
cyber permissions auto reset [OPTIONS]
```

## `cyber permissions test`

Evaluate a tool call or shell command as a Session would, without running it

```text
cyber permissions test [OPTIONS] [-- <COMMAND>...]
```

**Arguments:**

- `[COMMAND]...`: A shell command, split into commands as `bash` splits it.

**Options:**

- `--agent <AGENT>`
- `--tool <TOOL>`
- `--resource <RESOURCE>`

## `cyber db`

Inspect the local database

```text
cyber db [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`path`](#cyber-db-path): Print the database path
- [`query`](#cyber-db-query): Run a read-only SQL query (TSV, or JSON with --format json)
- [`backup`](#cyber-db-backup): Copy the database while the server runs (online backup). With --artifacts, write a bundle directory with tool output, snapshots and a manifest of hashes
- [`verify`](#cyber-db-verify): Check a backup bundle against its manifest and the database's integrity
- [`restore`](#cyber-db-restore): Replace the database (and bundle artifacts) from a backup; the server must be stopped
- [`vacuum`](#cyber-db-vacuum): Rebuild the database to reclaim space; the server must be stopped

## `cyber db path`

Print the database path

```text
cyber db path [OPTIONS]
```

## `cyber db query`

Run a read-only SQL query (TSV, or JSON with --format json)

```text
cyber db query [OPTIONS] <SQL>
```

**Arguments:**

- `<SQL>`

## `cyber db backup`

Copy the database while the server runs (online backup). With --artifacts, write a bundle directory with tool output, snapshots and a manifest of hashes

```text
cyber db backup [OPTIONS] <PATH>
```

**Arguments:**

- `<PATH>`

**Options:**

- `--artifacts`

## `cyber db verify`

Check a backup bundle against its manifest and the database's integrity

```text
cyber db verify [OPTIONS] <PATH>
```

**Arguments:**

- `<PATH>`

## `cyber db restore`

Replace the database (and bundle artifacts) from a backup; the server must be stopped

```text
cyber db restore [OPTIONS] <PATH>
```

**Arguments:**

- `<PATH>`

## `cyber db vacuum`

Rebuild the database to reclaim space; the server must be stopped

```text
cyber db vacuum [OPTIONS]
```

## `cyber plugins`

Install, list, enable, disable, update and remove plugins

```text
cyber plugins [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`install`](#cyber-plugins-install): Install a plugin from a git URL (`git+<url>[#ref]`) or a local directory
- [`list`](#cyber-plugins-list): List plugins of every scope with their state; every plugin is `unsigned`
- [`info`](#cyber-plugins-info): Show one plugin's state, source, digest and components
- [`update`](#cyber-plugins-update): Fetch a plugin (or every plugin) again from its source and record the new digest
- [`remove`](#cyber-plugins-remove): Remove a plugin from a scope (default: every scope listing it) and revoke its trust
- [`enable`](#cyber-plugins-enable): Enable a listed plugin, installing it at its pinned commit and asking to trust it
- [`disable`](#cyber-plugins-disable): Disable a plugin in a scope (default: every scope listing it)
- [`validate`](#cyber-plugins-validate): Check a plugin directory's manifest and components without installing it
- [`configure`](#cyber-plugins-configure): Set a plugin's `userConfig` settings: `--set key=value` for ordinary values, or answer a prompt for each (secrets without echo, stored in the OS keyring)

## `cyber plugins install`

Install a plugin from a git URL (`git+<url>[#ref]`) or a local directory

```text
cyber plugins install [OPTIONS] <SPEC>
```

**Arguments:**

- `<SPEC>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`. Default: `user`.
- `--ref <REF>`: The git branch, tag or commit.
- `--path <DIR>`: The plugin's directory inside the git repository.
- `--force`: Install even when `engines.cyber` excludes this version.

## `cyber plugins list`

List plugins of every scope with their state; every plugin is `unsigned`

```text
cyber plugins list [OPTIONS]
```

## `cyber plugins info`

Show one plugin's state, source, digest and components

```text
cyber plugins info [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber plugins update`

Fetch a plugin (or every plugin) again from its source and record the new digest

```text
cyber plugins update [OPTIONS] [NAME]
```

**Arguments:**

- `[NAME]`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`.
- `--yes`: Trust the new content without asking.
- `--force`

## `cyber plugins remove`

Remove a plugin from a scope (default: every scope listing it) and revoke its trust

```text
cyber plugins remove [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`.

## `cyber plugins enable`

Enable a listed plugin, installing it at its pinned commit and asking to trust it

```text
cyber plugins enable [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`.
- `--yes`: Trust the plugin without asking.
- `--force`

## `cyber plugins disable`

Disable a plugin in a scope (default: every scope listing it)

```text
cyber plugins disable [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--scope <SCOPE>`: Values: `user`, `project`, `local`.

## `cyber plugins validate`

Check a plugin directory's manifest and components without installing it

```text
cyber plugins validate [OPTIONS] <PATH>
```

**Arguments:**

- `<PATH>`

## `cyber plugins configure`

Set a plugin's `userConfig` settings: `--set key=value` for ordinary values, or answer a prompt for each (secrets without echo, stored in the OS keyring)

```text
cyber plugins configure [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

**Options:**

- `--set <KEY=VALUE>`

## `cyber hooks`

Review hook definitions and individual handler trust

```text
cyber hooks [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`list`](#cyber-hooks-list): List resolved definitions, file origins, digests and individual trust state
- [`test`](#cyber-hooks-test): Run matching hooks against a synthetic event without creating a Session
- [`history`](#cyber-hooks-history): Inspect committed Session execution receipts without running hooks or recovery
- [`trust`](#cyber-hooks-trust): Approve a currently resolved project/local handler after inspecting its digest
- [`untrust`](#cyber-hooks-untrust): Revoke a handler digest, including an obsolete definition

## `cyber hooks list`

List resolved definitions, file origins, digests and individual trust state

```text
cyber hooks list [OPTIONS]
```

## `cyber hooks test`

Run matching hooks against a synthetic event without creating a Session

```text
cyber hooks test [OPTIONS] <EVENT>
```

**Arguments:**

- `<EVENT>`

**Options:**

- `--payload <FILE>`: JSON object of event fields; trusted envelope fields cannot be supplied.

## `cyber hooks history`

Inspect committed Session execution receipts without running hooks or recovery

```text
cyber hooks history [OPTIONS] --session <SESSION>
```

**Options:**

- `--session <SESSION>`
- `--limit <LIMIT>`: Default: `50`.
- `--cursor <CURSOR>`

## `cyber hooks trust`

Approve a currently resolved project/local handler after inspecting its digest

```text
cyber hooks trust [OPTIONS] --digest <DIGEST>
```

**Options:**

- `--digest <DIGEST>`

## `cyber hooks untrust`

Revoke a handler digest, including an obsolete definition

```text
cyber hooks untrust [OPTIONS] --digest <DIGEST>
```

**Options:**

- `--digest <DIGEST>`

## `cyber sandbox`

Test, explain or run a command under the effective OS sandbox policy

```text
cyber sandbox [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`test`](#cyber-sandbox-test): Run read, write and network probes under the effective policy
- [`explain`](#cyber-sandbox-explain): Print the effective writable roots, network mode, domains and masked variables
- [`run`](#cyber-sandbox-run): Run one command under the effective policy

## `cyber sandbox test`

Run read, write and network probes under the effective policy

```text
cyber sandbox test [OPTIONS]
```

**Options:**

- `--policy <POLICY>`: A policy: read-only, workspace-write or full-access.

## `cyber sandbox explain`

Print the effective writable roots, network mode, domains and masked variables

```text
cyber sandbox explain [OPTIONS]
```

**Options:**

- `--policy <POLICY>`: A policy: read-only, workspace-write or full-access.

## `cyber sandbox run`

Run one command under the effective policy

```text
cyber sandbox run [OPTIONS] -- <COMMAND>...
```

**Arguments:**

- `<COMMAND>...`: The command and its arguments, after `--`.

**Options:**

- `--policy <POLICY>`: A policy: read-only, workspace-write or full-access.

## `cyber trust`

Inspect and approve repository-controlled configuration for this checkout

```text
cyber trust [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`inspect`](#cyber-trust-inspect): Show the checkout, the digest of its security-sensitive definitions and their state
- [`approve`](#cyber-trust-approve): Approve exactly the inspected definitions
- [`revoke`](#cyber-trust-revoke): Revoke this checkout's approval

## `cyber trust inspect`

Show the checkout, the digest of its security-sensitive definitions and their state

```text
cyber trust inspect [OPTIONS]
```

## `cyber trust approve`

Approve exactly the inspected definitions

```text
cyber trust approve [OPTIONS] --digest <DIGEST>
```

**Options:**

- `--digest <DIGEST>`: Digest printed by `cyber trust inspect`.

## `cyber trust revoke`

Revoke this checkout's approval

```text
cyber trust revoke [OPTIONS]
```

## `cyber exec`

Run one prompt non-interactively (alias: cyber -p)

```text
cyber exec [OPTIONS] [PROMPT]...
```

**Arguments:**

- `[PROMPT]...`: The prompt; stdin is appended when it is not a terminal.

**Options:**

- `--worktree [<NAME>]`: Start a new Session in a managed worktree; omit NAME to generate one.
- `--continue`: Reuse the most recent root Session of the Location.
- `--session <SESSION>`: Reuse a Session by ID or name.
- `--fork`: Fork the selected Session first (needs --continue or --session)
- `--name <NAME>`: Name the new Session.
- `--ephemeral`: Run in memory: nothing is written to the shared database or snapshots.
- `--embedded`: Run a private in-process server instead of the background service.
- `--attach <ATTACH>`: Target a remote server URL.
- `--password <PASSWORD>`: Password for --attach (else CYBER_SERVER_PASSWORD)
- `--agent <AGENT>`
- `--auto`: Use `auto` mode.
- `--yolo`: Use `bypass` mode: everything runs without approval. The server refuses it unless commands run in the OS sandbox or a container, or with --dangerously-bypass-permissions.
- `--dangerously-bypass-permissions`: Allow `bypass` mode without a sandbox or a container (needs --embedded or --ephemeral: a shared server cannot take it for one run)
- `--dangerously-skip-permissions`: Claude Code's name for running without prompts: `bypass` mode plus --dangerously-bypass-permissions, so every action runs without approval and, outside a sandbox or a container, without OS confinement. Needs --embedded or --ephemeral: a shared server cannot take the override for one run (there, use --mode bypass where its sandbox or a container already admits it). An organization policy that disables `bypass` still refuses it.
- `--non-interactive-rules`: Add the never-ask rules to a reused Session too.
- `--accept-messages`: Let other Sessions message the Session this run creates (by default it refuses them with `non_interactive`)
- `--command <COMMAND>`: Run a skill or custom command with the prompt as its arguments.
- `--workflow <NAME|PATH>`: Run a Workflow (a saved name or a `.js` module path) instead of a prompt; its return value is the result.
- `--args <JSON>`: The Workflow's `args` as a JSON object.
- `--file <FILES>`: Attach a file (repeatable)
- `--max-turns <MAX_TURNS>`: With --goal: each Goal's `max_turns` budget; otherwise the Session's step ceiling.
- `--goal <CONDITION>`: Work until this Goal is met (repeatable: a queue, in order). The exit code is 0 met, 3 impossible, 4 budget exceeded, 1 otherwise, for the first non-met Goal.
- `--goal-check <COMMAND>`: A command that must pass for the --goal before it to count as met.
- `--max-tokens <MAX_TOKENS>`
- `--max-cost <MAX_COST>`: Stop after this cost in USD.
- `--timeout <TIMEOUT>`: Stop after this long, e.g. 30m, 90s.
- `--verbose`: Print tool progress on stderr (text format)
- `--quiet`: No non-result output on stderr.
- `--thinking`: Include reasoning events (stream-json)
- `--sandbox <SANDBOX>`: Sandbox policy: read-only, workspace-write or full-access (embedded runs)
- `--fail-on-deny`: Exit with code 5 when any action was denied.
- `--trust-project-hooks`: Run the currently resolved project/local hooks for this run only, without approving them for later runs (needs --embedded or --ephemeral)

## `cyber attach`

Open the TUI on a Session of a paired machine: cyber attach <session>@<peer>

```text
cyber attach [OPTIONS] <TARGET>
```

**Arguments:**

- `<TARGET>`: `<session>@<peer>`: a Session (ID or name) of a paired machine.

## `cyber review`

Review uncommitted changes, a range, a commit or a pull request

```text
cyber review [OPTIONS] [TARGET]...
```

**Arguments:**

- `[TARGET]...`: What to review: nothing for uncommitted changes, `<base>..<head>`, a commit, or `pr <number>`.

**Options:**

- `--worktree [<NAME>]`: Start a new Session in a managed worktree; omit NAME to generate one.
- `--continue`: Reuse the most recent root Session of the Location.
- `--session <SESSION>`: Reuse a Session by ID or name.
- `--ephemeral`: Run in memory: nothing is written to the shared database or snapshots.
- `--embedded`: Run a private in-process server instead of the background service.
- `--attach <ATTACH>`: Target a remote server URL.
- `--password <PASSWORD>`: Password for --attach (else CYBER_SERVER_PASSWORD)
- `--max-tokens <MAX_TOKENS>`
- `--max-cost <MAX_COST>`: Stop after this cost in USD.
- `--timeout <TIMEOUT>`: Stop after this long, e.g. 30m, 90s.
- `--fail-on-deny`: Exit with code 5 when any action was denied.

## `cyber pr`

Check out a pull request and resume the Session linked to it

```text
cyber pr [OPTIONS] <NUMBER|URL>
```

**Arguments:**

- `<NUMBER|URL>`: The pull request number or URL.

## `cyber git`

Generate commit and pull request text with the small model

```text
cyber git [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`commit-msg`](#cyber-git-commit-msg): Print a conventional commit message for the staged changes
- [`pr-description`](#cyber-git-pr-description): Print a pull request title and body for BASE..HEAD

## `cyber git commit-msg`

Print a conventional commit message for the staged changes

```text
cyber git commit-msg [OPTIONS]
```

## `cyber git pr-description`

Print a pull request title and body for BASE..HEAD

```text
cyber git pr-description [OPTIONS]
```

**Options:**

- `--base <BASE>`: The base branch or commit (default: origin/HEAD, else main, else master)

## `cyber serve`

Run the server in the foreground

```text
cyber serve [OPTIONS]
```

**Options:**

- `--hostname <HOSTNAME>`: Address to bind (default 127.0.0.1) Default: `127.0.0.1`.
- `--port <PORT>`: TCP port (default 4747, falling back to a free port)
- `--socket <SOCKET>`: Unix socket path (default <state>/cyber.sock)
- `--no-tcp`: Do not listen on TCP.
- `--register`: Register in server.json so clients find this server.
- `--sandbox <SANDBOX>`: Sandbox policy for model-run commands: read-only, workspace-write or full-access.
- `--mdns`: Publish the peer listener (`peers.listen`) over mDNS, as `peers.mdns` does. Only a listener off loopback is published; the main API never is.
- `--stdio`: Speak JSON-RPC over stdin and stdout.
- `--dangerously-bypass-permissions`: Allow `bypass` mode without a sandbox or a container. Everything the model runs executes without approval or OS confinement.

## `cyber service`

Manage the background server

```text
cyber service [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`start`](#cyber-service-start): Start the background server, or reuse a healthy one
- [`stop`](#cyber-service-stop): Stop the background server
- [`restart`](#cyber-service-restart): Stop, then start
- [`status`](#cyber-service-status): Show the registered server and whether it is healthy
- [`password`](#cyber-service-password): Print the local server password, or replace it (restarting the server)

## `cyber service start`

Start the background server, or reuse a healthy one

```text
cyber service start [OPTIONS]
```

## `cyber service stop`

Stop the background server

```text
cyber service stop [OPTIONS]
```

## `cyber service restart`

Stop, then start

```text
cyber service restart [OPTIONS]
```

## `cyber service status`

Show the registered server and whether it is healthy

```text
cyber service status [OPTIONS]
```

## `cyber service password`

Print the local server password, or replace it (restarting the server)

```text
cyber service password [OPTIONS] [VALUE]
```

**Arguments:**

- `[VALUE]`

## `cyber api`

Send one request to the server API

```text
cyber api [OPTIONS] <TARGET>...
```

**Arguments:**

- `<TARGET>...`: An operation ID (`v1.session.list`) or `METHOD /path`.

**Options:**

- `-d, --data <DATA>`: Request body (JSON by default)
- `-H, --header <HEADERS>`: Extra header as `Name: value` (repeatable, up to 100)
- `--param <PARAMS>`: Path or query parameter as `key=value` (repeatable)

## `cyber peers`

Pair with other machines' cyber servers to message their Sessions

```text
cyber peers [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`invite`](#cyber-peers-invite): Let another machine send to this one: create a grant and print its pairing code once. Needs `peers.listen` in user config
- [`add`](#cyber-peers-add): Pair with another machine from the code its `cyber peers invite` printed: test it, store the token in the OS keyring and add the remote to user config
- [`rm`](#cyber-peers-rm): Remove a remote and its stored token
- [`list`](#cyber-peers-list): Remotes (state and latency) and grants of this machine
- [`test`](#cyber-peers-test): Call a remote's `hello`: machine, version, scopes and latency; exit 1 with the error tag on failure
- [`revoke`](#cyber-peers-revoke): End a grant: the peer's next request is refused
- [`discover`](#cyber-peers-discover): Peer listeners published on this network over mDNS: machine, address, fingerprint and whether each is a remote already. It only lists: pairing still needs a code

## `cyber peers invite`

Let another machine send to this one: create a grant and print its pairing code once. Needs `peers.listen` in user config

```text
cyber peers invite [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`: The other machine's name here: its senders appear as `<session>@<name>`.

**Options:**

- `--scope <SCOPES>`: What the grant allows (repeatable); default `message`. Values: `message`, `attach`.
- `--expires <EXPIRES>`: End the grant after this long: `90d`, `12h` or `30m`.
- `--rotate-key`: Make a new listener key first; every existing pairing must be redone.
- `--host <HOST>`: The host the code names (default: the bound IP, or this host's LAN name)

## `cyber peers add`

Pair with another machine from the code its `cyber peers invite` printed: test it, store the token in the OS keyring and add the remote to user config

```text
cyber peers add [OPTIONS] <NAME> <CODE>
```

**Arguments:**

- `<NAME>`: The name to send to it by (`<session>@<name>`)
- `<CODE>`: The pairing code `cyber peers invite` printed on the other machine.

**Options:**

- `--token-env <NAME>`: Keep the token out of the keyring: config gets `{env:NAME}`, and `NAME` must hold the code's token wherever the server runs (headless machines)

## `cyber peers rm`

Remove a remote and its stored token

```text
cyber peers rm [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber peers list`

Remotes (state and latency) and grants of this machine

```text
cyber peers list [OPTIONS]
```

**Options:**

- `--json`: Print JSON.

## `cyber peers test`

Call a remote's `hello`: machine, version, scopes and latency; exit 1 with the error tag on failure

```text
cyber peers test [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber peers revoke`

End a grant: the peer's next request is refused

```text
cyber peers revoke [OPTIONS] <NAME>
```

**Arguments:**

- `<NAME>`

## `cyber peers discover`

Peer listeners published on this network over mDNS: machine, address, fingerprint and whether each is a remote already. It only lists: pairing still needs a code

```text
cyber peers discover [OPTIONS]
```

**Options:**

- `--timeout <TIMEOUT>`: How long to listen: `3s`, `500ms` or `1m`. Default: `3s`.
- `--json`: Print JSON.

## `cyber messages`

Send messages between Sessions and read their message history

```text
cyber messages [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`send`](#cyber-messages-send): Send a message to a Session by ID or name. Exits 0 when delivered or held, 1 when refused (the reason on stderr) or when a peer cannot be reached, 2 for an ambiguous or unknown address, an unconfigured peer or a text over 16 KiB, and 6 for an address that needs a Cyber Account
- [`list`](#cyber-messages-list): The message history of a Session (default: the Location's most recent one), or with `--reachable` the Sessions a message can be sent to

## `cyber messages send`

Send a message to a Session by ID or name. Exits 0 when delivered or held, 1 when refused (the reason on stderr) or when a peer cannot be reached, 2 for an ambiguous or unknown address, an unconfigured peer or a text over 16 KiB, and 6 for an address that needs a Cyber Account

```text
cyber messages send [OPTIONS] <TO> [TEXT]
```

**Arguments:**

- `<TO>`: The target: a `ses_` ID, a subagent name, a Session name, or `<session>@<peer>` for a Session of a paired machine (which needs `--from`)
- `[TEXT]`: The message text; `-` (or nothing, with a pipe) reads it from stdin.

**Options:**

- `--from <FROM>`: Send as this Session (ID, exact title or unique ID prefix) instead of as the `client:cli` client; a message to a peer Session (`<session>@<peer>`) needs it.
- `--notify-when-idle`: Also notify the `--from` Session once when the target next goes idle.

## `cyber messages list`

The message history of a Session (default: the Location's most recent one), or with `--reachable` the Sessions a message can be sent to

```text
cyber messages list [OPTIONS]
```

**Options:**

- `--session <SESSION>`: The Session whose history to show (ID, exact title or unique ID prefix)
- `--direction <DIRECTION>`: Only received (`in`) or sent (`out`) messages. Values: `in`, `out`.
- `--limit <LIMIT>`: At most this many records, newest first (1 to 200) Default: `50`.
- `--reachable`: List the Sessions a message can be sent to instead of a history.
- `--from <FROM>`: With `--reachable`: list as this Session (ID, exact title or unique ID prefix) instead of as the `client:cli` client.

## `cyber doctor`

Check configuration, skills, commands, credentials, catalog, database, sandbox, tools and server

```text
cyber doctor [OPTIONS]
```

## `cyber stats`

Usage statistics: activity, streaks, tokens, cost and per-model totals

```text
cyber stats [OPTIONS]
```

**Options:**

- `--days <DAYS>`: Only the last N local days, today included (1 to 371)
- `--project <DIR>`: Only Sessions of the project containing this directory.
- `--models`: One row per model.
- `--tools`: Tool calls by name.
- `--by <GROUP>`: One row per usage origin (`user`, `session`, `client`, `job`, `mcp`, ...) Values: `origin`.

## `cyber upgrade`

Upgrade cyber to the channel's latest version or to VERSION

```text
cyber upgrade [OPTIONS] [VERSION]
```

**Arguments:**

- `[VERSION]`: Version to install (`1.4.0` or `v1.4.0`); defaults to the channel's latest.

**Options:**

- `--method <METHOD>`: Install method instead of the detected one: script, brew, npm, pnpm, bun, cargo, scoop, winget, aur, nix, docker or unknown.
- `--channel <CHANNEL>`: Channel to read the latest version from; defaults to the build's (`dev` reads `stable`) Values: `stable`, `beta`, `nightly`.
- `--dry-run`: Print what would be done (target version, method, action) and change nothing.

## `cyber uninstall`

Remove cyber: its binary (script installs), PATH lines and directories

```text
cyber uninstall [OPTIONS]
```

**Options:**

- `--keep-config`: Keep the configuration directory.
- `--keep-data`: Keep the data directory (sessions, memory, stored logins) and the keyring entries.
- `--dry-run`: List what would be removed, with sizes, and change nothing.
- `--force`: Remove without asking, and go on when the background server cannot be stopped.

## `cyber eval`

Run evaluation manifests and write reports

```text
cyber eval [OPTIONS] <COMMAND>
```

**Subcommands:**

- [`run`](#cyber-eval-run): Run a coding manifest and write a report

## `cyber eval run`

Run a coding manifest and write a report

```text
cyber eval run [OPTIONS] --output <OUTPUT> <MANIFEST>
```

**Arguments:**

- `<MANIFEST>`: Manifest path, e.g. eval/manifests/suite-coding-v1.json.

**Options:**

- `--output <OUTPUT>`: Directory for report.json, report.md and per-trial logs.
- `-m, --model <MODEL>`: Model as provider/model[#variant]; required for suites, overrides live manifests.
- `--trials <TRIALS>`: Trials per task (the release baseline uses 3) Default: `3`.
- `--task <TASKS>`: Only these task IDs (repeatable)
- `--jobs <JOBS>`: Trials run at the same time. Default: `4`.
- `--keep-workspaces`: Keep trial workspaces under <output>/workspaces for inspection.
