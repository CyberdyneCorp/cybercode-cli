## MODIFIED Requirements

### Requirement: XDG directory layout
(P0) The system SHALL use XDG base directories with app name `cyber`:
- data `$XDG_DATA_HOME/cyber` (default `~/.local/share/cyber`)
- config `$XDG_CONFIG_HOME/cyber`
- state `$XDG_STATE_HOME/cyber`
- cache `$XDG_CACHE_HOME/cyber`

Derived directories SHALL be `<data>/log`, `<data>/tool-output`, `<data>/jobs`, `<data>/snapshot`, `<data>/memory`, `<data>/worktrees`, `<cache>/bin`, `<cache>/models`, `<cache>/plugins` and `<os tmpdir>/cyber`, and SHALL be created at startup. On macOS and Windows the same XDG defaults under the home directory SHALL be used unless the XDG variables are set. `CYBER_HOME` SHALL relocate all of them under one root; an explicitly set `XDG_*` variable, `CYBER_CONFIG_DIR` or `CYBER_DB` SHALL take precedence over `CYBER_HOME` for the directory or file it names.

#### Scenario: CYBER_HOME relocation
- **WHEN** `CYBER_HOME=/opt/cyber-home` is set
- **THEN** data, config, state and cache live under `/opt/cyber-home/{data,config,state,cache}`

#### Scenario: Specific variable wins over CYBER_HOME
- **WHEN** `CYBER_HOME=/opt/cyber-home` and `CYBER_CONFIG_DIR=/etc/cyber-user` are both set
- **THEN** config is read from `/etc/cyber-user` and the other directories live under `/opt/cyber-home`

### Requirement: Retention and garbage collection
(P1) A background GC SHALL run 2 minutes after server start and then every 6 hours. It SHALL delete:
- unpinned managed tool-output files older than 7 days (the only cleanup of `<data>/tool-output`)
- archived Sessions older than `storage.retention.archived_days` (default 90)
- finished background jobs and their output files older than 7 days
- finished workflow run agent transcripts older than 30 days, keeping run summaries

Once every 24 hours it SHALL also run `git gc --prune=7.days` on snapshot repositories and prune file-copy snapshot objects, as specified by `snapshots-checkpoints`. Setting a retention value to `0` SHALL disable that rule.

#### Scenario: Tool output cleanup
- **WHEN** a file in `<data>/tool-output` is 8 days old during a GC pass
- **THEN** it is deleted

### Requirement: Concurrent process safety
(P0) Multiple `cyber` processes SHALL safely read the shared database through WAL and `busy_timeout`. Only the registered server, which holds the advisory exclusive lock `<state>/server.lock`, SHALL run Drains and application writes against the shared database. Other processes SHALL route execution requests and mutations to the registered server over its API. An `--embedded` process SHALL use a private database and SHALL NOT open the shared database for writing.

#### Scenario: Second server refused
- **WHEN** a server is running and the user starts `cyber serve --register` in another terminal
- **THEN** the second process fails with `another cyber server is registered at <url>` and exits 1

#### Scenario: Embedded process stays private
- **WHEN** `cyber exec --embedded` runs while a server is registered
- **THEN** it opens only its private database and the registered server's lock and database are untouched

### Requirement: Local writer ownership and backpressure
(P0) The registered server SHALL own application writes through one bounded writer queue (default 1024 transactions), with separate read connections. CLI mutations SHALL route through that owner; stopped-server maintenance (`cyber db vacuum|restore`) SHALL acquire the lock. Embedded processes SHALL apply the same queue and durability settings to their private database. Transactions SHALL contain no provider calls, tool execution, user waits or network I/O. Queue admission SHALL time out after 5 seconds with retryable `StorageBusyError` (HTTP 503), without acknowledging an uncommitted prompt. Readers SHALL release transactions between replay pages. Queue depth, commit latency, busy errors and WAL size SHALL be observable.

#### Scenario: Writer saturation
- **WHEN** the writer queue is full for 5 seconds
- **THEN** admission returns StorageBusyError, no receipt is acknowledged, and the client can retry with the same message ID
