## ADDED Requirements

### Requirement: Session search command
(P1) `cyber sessions search <query>` SHALL search Session transcripts through `GET /api/v1/sessions/search` of the registered server, starting it when needed. It SHALL accept these flags:
- `--scope project|directory|all` (default `project`);
- `--all` as a short form of `--scope all`;
- `--exclude-archived`;
- `--sort relevance|recent`;
- `--limit N` (1 to 100, default 20);
- `--format table|json`.

**Table output.** The table SHALL print one Session per row (ID, title, relative update time) with its best snippet indented beneath. Highlights SHALL be bold on a TTY and printed without markup otherwise.

**JSON output.** `--format json` SHALL print the response's `data` unchanged.

**Messages.** No match SHALL print `No matching Sessions` to stderr and exit 0. While the index backfill runs, the command SHALL print `index incomplete: backfilling <N>%` to stderr. An invalid query SHALL exit 2 with the server's reason.

`cyber sessions list --search <text>` SHALL remain a title filter.

#### Scenario: Find the Session that fixed a bug
- **WHEN** the user runs `cyber sessions search "flaky login"` in `/repo`
- **THEN** stdout lists the matching Sessions of the project, best first, each with a snippet such as `…the flaky login test was caused by a shared tmp dir…`

#### Scenario: JSON for scripts
- **WHEN** the user runs `cyber sessions search ECONNRESET --all --format json`
- **THEN** stdout is the route's `data` object with `sessions` and `index`

#### Scenario: Unsupported format
- **WHEN** the user runs `cyber sessions search login --format html`
- **THEN** the command fails with exit code 2 and says that `--format html` is only for `cyber sessions export`
