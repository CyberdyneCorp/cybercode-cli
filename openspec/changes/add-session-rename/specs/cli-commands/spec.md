## MODIFIED Requirements

### Requirement: Resource command groups
(P0) Resource groups SHALL use consistent verbs: `list` (alias `ls`), `show <id>`, `delete <id>` (alias `rm`) and, where applicable, `create`, `stop`, `resume`. These groups SHALL exist with at least these subcommands:
- `sessions` (list, show, delete, export, import, fork, rename, rewind, repair-context, share, unshare)
- `agents` (list, show, create), `models` (list, refresh), `providers` (list, login, logout, use), `skills` (list, show), `commands` (list), `memory` (list, show, edit, delete, path)
- `worktree` (list, remove, prune), `review`, `pr <number>`, `git` (commit-msg, pr-description), `web`
- `workflows` (run, list, runs, show, pause, resume, stop, logs), `goals` (set, list, show, pause, resume, clear), `loops` (add, list, show, pause, resume, run, rm, promote), `teams` (list, show, stop, resume), `routines` (create, list, show, edit, fire, pause, resume, rm, runs), `handoff`, `teleport <session>`, `apply <session>`
- `tokens` (create, list, revoke), `peers` (list, add, rm, test, discover), `remote` (enable, disable, pair, devices, revoke, log), `runners` (list, use, logs), `runner` (start, drain), `messages` (list, send), `channels` (add, list, show, test, rotate-secret, log, disable, enable, rm), `orchestrator` (serve), `relay` (serve), `share` (serve), `github` (install), `acp`, `ide` (install)
- `mcp` (add, list, get, remove, auth, logout, debug, serve), `plugins` (install, list, info, update, remove, enable, disable, validate, marketplace, eval), `hooks` (list, trust, untrust, test), `permissions` (list, revoke, test, auto), `trust` (inspect, approve, revoke), `sandbox` (status, test, explain, run), `features` (list, enable, disable), `lsp` (status), `fmt` (status), `env` (create, list, edit, rm), `import` (claude, codex, opencode, auto, --detect)
- `db` (query, path, vacuum, backup, restore, encrypt, decrypt), `telemetry` (show), `eval` (run)

A singular alias of `sessions`, a standalone export command and a session-file import under `import` SHALL NOT exist. A Session argument of `cyber sessions`, `cyber -r` and `cyber exec --session` SHALL resolve, among the Location's Sessions, as an ID, then an exact title, then a unique ID prefix; a title or prefix that matches several Sessions SHALL exit 2 and list their IDs. `cyber sessions delete` SHALL require `--yes`.

#### Scenario: List output formats
- **WHEN** the user runs `cyber sessions list --format json`
- **THEN** stdout is a JSON array of objects with `id`, `title`, `directory`, `updated_at` and `status`

#### Scenario: Table paging
- **WHEN** `cyber sessions list` writes a table to a TTY without `--limit`
- **THEN** output is paged through `$PAGER` (default `less -RS`)

#### Scenario: Resume by title
- **WHEN** a Session of the Location is titled `Auth refactor` and the user runs `cyber exec --session "Auth refactor" "go on"`
- **THEN** the prompt is admitted to that Session

#### Scenario: Ambiguous title
- **WHEN** two Sessions of the Location are titled `same` and the user runs `cyber sessions show same`
- **THEN** the CLI exits 2 and lists both Session IDs
