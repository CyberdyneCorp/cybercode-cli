## MODIFIED Requirements

### Requirement: Attach command
(P0) `cyber attach <url|session>` SHALL open the TUI against a running server or session. For a URL it SHALL accept `--password`/`-p` and `--username`/`-u`. For `<session>@<peer>` it SHALL attach through the configured peer (see `cross-session-messaging` → Peer attach). For a session reachable through the Relay it SHALL require a logged-in Cyber Account (see `remote-control`).

#### Scenario: Attach with password
- **WHEN** the user runs `cyber attach http://10.0.0.5:4747 -p s3cret`
- **THEN** the TUI connects with HTTP Basic credentials `cyber:s3cret`

#### Scenario: Attach to a peer Session
- **WHEN** the user runs `cyber attach migration@desk` and `desk` is a configured remote with an `attach` grant
- **THEN** the TUI opens that Session through the peer listener after `desk` approves `remote.attach`

### Requirement: Resource command groups
(P0) Resource groups SHALL use consistent verbs: `list` (alias `ls`), `show <id>`, `delete <id>` (alias `rm`) and, where applicable, `create`, `stop`, `resume`. These groups SHALL exist with at least these subcommands:
- `sessions` (list, show, delete, export, import, fork, rename, rewind, repair-context, share, unshare)
- `agents` (list, show, create), `models` (list, refresh), `providers` (list, login, logout, use), `skills` (list, show), `commands` (list), `memory` (list, show, edit, delete, path)
- `worktree` (list, remove, prune), `review`, `pr <number>`, `git` (commit-msg, pr-description), `web`
- `workflows` (run, list, runs, show, pause, resume, stop, logs), `goals` (set, list, show, pause, resume, clear), `loops` (add, list, show, pause, resume, run, rm, promote), `teams` (list, show, stop, resume), `routines` (create, list, show, edit, fire, pause, resume, rm, runs), `handoff`, `teleport <session>`, `apply <session>`
- `tokens` (create, list, revoke), `peers` (list, add, rm, test, discover, invite, revoke), `remote` (enable, disable, pair, devices, revoke, log), `runners` (list, use, logs), `runner` (start, drain), `messages` (list, send), `channels` (add, list, show, test, rotate-secret, log, disable, enable, rm), `orchestrator` (serve), `relay` (serve), `share` (serve), `github` (install), `acp`, `ide` (install)
- `mcp` (add, list, get, remove, auth, logout, debug, serve), `plugins` (install, list, info, update, remove, enable, disable, validate, marketplace, eval), `hooks` (list, trust, untrust, test), `permissions` (list, revoke, test, auto), `trust` (inspect, approve, revoke), `sandbox` (status, test, explain, run), `features` (list, enable, disable), `lsp` (status), `fmt` (status), `env` (create, list, edit, rm), `import` (claude, codex, opencode, auto, --detect)
- `db` (query, path, vacuum, backup, restore, encrypt, decrypt), `telemetry` (show), `eval` (run)

A singular alias of `sessions`, a standalone export command and a session-file import under `import` SHALL NOT exist.

#### Scenario: List output formats
- **WHEN** the user runs `cyber sessions list --format json`
- **THEN** stdout is a JSON array of objects with `id`, `title`, `directory`, `updated_at` and `status`

#### Scenario: Table paging
- **WHEN** `cyber sessions list` writes a table to a TTY without `--limit`
- **THEN** output is paged through `$PAGER` (default `less -RS`)

## ADDED Requirements

### Requirement: Peers command
(P2) `cyber peers` SHALL manage direct peers: `list [--json]` prints remotes (name, URL, fingerprint prefix, state, latency) and grants (name, scopes, created, last used, expiry), flagging a grant with no remote of the same name; `add <name> <code>`, `rm <name>` (removes the remote and its stored token), `test <name>` (prints machine, version, scopes and latency, exit 1 with the error tag on failure), `discover`, `invite <name>` and `revoke <name>`. Tokens SHALL be printed only by `invite`, once.

#### Scenario: Grant without a way back
- **WHEN** `desk` has a grant `laptop` but no remote named `laptop`
- **THEN** `cyber peers list` on `desk` marks `laptop` `can receive, cannot reply` and prints the command to run on `laptop`
