## ADDED Requirements

### Requirement: File browsing routes
(P1) The server SHALL provide, in the `fs` route group (`v1.fs.list`, `v1.fs.search`, `v1.fs.content`) and scoped to the request's Location, `GET /api/v1/files?path=<rel>&hidden=<bool>`, which returns one directory level as `entries[]` of `{name, path, kind: file|directory|symlink, size, git_status, ignored, target?, read: allow|ask|deny}`, plus `hidden` (dot and ignored entries left out), `changed` (the project's changed-file count) and `root` (the canonical project root that entry paths are relative to). It SHALL sort directories first and then by name, and SHALL cap results at 2,000 entries with `truncated: true`. A directory's `git_status` SHALL be `M` when any path below it changed. It SHALL also provide `GET /api/v1/files/search?directory=&query=&limit=` for fuzzy path search over non-ignored files, and `GET /api/v1/files/content?directory=&path=` returning `{text | binary: true, size, language, lines, truncated}`. These routes SHALL take `session_id` when present, so that the Session's permission rules and Mode apply; without it the Location's rules apply in `default` mode, and an `ask` SHALL be refused because no Session can be asked. Listing a level SHALL require `list` on it. A search SHALL require `list` on the project root and SHALL leave out every hit whose directory the rules do not allow listing without asking. Only a regular file or a directory SHALL be opened; any other kind (such as a FIFO or a device) SHALL be refused with `400` before it is opened, and no open SHALL wait. Content SHALL be served only when the `read` decision is `allow` (an `ask` from a client with a Session SHALL go through the normal permission request), and otherwise the route SHALL return `403 ForbiddenError {reason: permission, decision}`. Paths SHALL be resolved relative to the project root and canonicalized; any path or symlink target outside the project SHALL return `403 ForbiddenError {reason: outside_project}`. Content and listings SHALL be read from the file or directory the decision was made on; when the path names another object by the time it is read (for example a symlink or hard link swapped in during an `ask`), the route SHALL return `403 ForbiddenError {reason: changed}` without its content. Content SHALL be capped at 1 MiB.

#### Scenario: Listing a level
- **WHEN** a client lists `crates` in a project with a `.gitignore`d `target/`
- **THEN** the response has the crate directories with git status, and `target` appears only with `hidden=true`, marked `ignored`

#### Scenario: Path escape refused
- **WHEN** a client requests content for `../../etc/passwd` or a symlink pointing outside the project
- **THEN** the response is `403` with `reason: outside_project` and no bytes of the file

#### Scenario: Denied file
- **WHEN** the Session's rules deny `read` for `secrets/key.pem`
- **THEN** the listing shows it with `read: deny`, and the content route returns `403` with `reason: permission`

## MODIFIED Requirements

### Requirement: Session and prompt routes
(P0) The server SHALL provide `GET|POST /api/v1/sessions`, `GET|PATCH|DELETE /api/v1/sessions/:sessionID`, `POST .../fork`, `POST .../prompt`, `POST .../interrupt`, `POST .../agent`, `POST .../model`, `POST .../mode`, `POST .../compact`, `POST .../rewind`, `POST .../revert/stage|clear|commit`, `GET .../messages`, `GET .../messages/:messageID`, `GET .../inbox`, `PATCH|DELETE .../inbox/:messageID`, `POST .../inbox/:messageID/release`, `POST .../inbox/:messageID/drop`, `GET .../context`, `GET .../diff`, `GET .../history` and `GET .../events`, and (P1) `GET|POST .../recap`. `POST .../prompt` SHALL accept `{ id?, parts, delivery?: "steer"|"queue"|"hold", resume?: boolean }`, durably admit the prompt, and respond `202` with `{ data: Admitted }`. It SHALL wake the Session's Drain unless `resume` is `false` or `delivery` is `hold`. The inbox release and drop routes SHALL be the only way to release or drop any held item, whatever its origin (user, cross-session message, channel event).

#### Scenario: Queue delivery
- **WHEN** a client posts a prompt with `delivery: "queue"` while the Session is running
- **THEN** the prompt is admitted and promoted only when the Session would otherwise become idle

#### Scenario: Hold delivery
- **WHEN** a prompt is admitted with `delivery: "hold"`
- **THEN** it is not promoted until a user approves it via `POST /api/v1/sessions/:sessionID/inbox/:messageID/release`

#### Scenario: Recap route
- **WHEN** a client calls `POST /api/v1/sessions/:sessionID/recap` on a Session with finished Turns
- **THEN** the response contains the recap text and the `through_seq` it covers
