## MODIFIED Requirements

### Requirement: Backup
(P0) `cyber db backup <file>` SHALL produce a consistent copy using the SQLite online backup API while the server runs, and `cyber db restore <file>` SHALL refuse to run while a server is registered. `cyber db backup <dir> --artifacts` SHALL write a bundle with the database, the tool-output and snapshot directories and a `manifest.json` listing the size and SHA-256 of every file, and SHALL leave no partial bundle on failure. `cyber db verify <dir>` SHALL report missing, changed and unexpected files and database integrity problems, and `cyber db restore` SHALL restore a bundle only after it verifies, keeping the replaced files as `*.pre-restore-<id>`.

#### Scenario: Online backup
- **WHEN** a Drain is writing events and the user runs `cyber db backup /tmp/b.db`
- **THEN** `/tmp/b.db` opens cleanly and passes `PRAGMA integrity_check`

#### Scenario: Tampered bundle
- **WHEN** a file in a backup bundle is changed after the backup and the user runs `cyber db restore <dir>`
- **THEN** the restore is refused with the changed path listed and the current database is left in place
