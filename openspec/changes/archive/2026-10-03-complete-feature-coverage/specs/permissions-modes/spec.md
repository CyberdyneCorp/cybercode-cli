## ADDED Requirements

### Requirement: Session ruleset API
(P0) The Session ruleset layer SHALL be settable by clients: `PUT /api/v1/sessions/:id/permissions/rules` with `{ rules: [{ action, resource, effect }] }` SHALL replace the Session's rules, `GET` SHALL return them with the effective merged ruleset, and the SDK SHALL expose `session.permissions.set(rules)` and `.get()`. `cyber exec --allow <action[:resource]>` and `--deny <action[:resource]>` SHALL populate this layer for the new Session. Session rules SHALL be recorded as a durable `session.permissions.updated.1` event and apply at the next Safe Boundary. They SHALL NOT widen explicit user/global denies, plan-mode restrictions, protected paths, workspace trust, sandbox boundaries or org policy, which remain ceilings.

#### Scenario: Script narrows a session
- **WHEN** an SDK client calls `session.permissions.set([{ action: "bash", resource: "*", effect: "deny" }])` on a running Session
- **THEN** from the next Safe Boundary every `bash` call is denied and the event is recorded

#### Scenario: Session rule cannot beat an org deny
- **WHEN** a Session rule allows `webfetch` for `*` and org policy denies `webfetch` for `*.internal.example.com`
- **THEN** fetching an internal host is still denied with reason `org policy`

### Requirement: Auto-mode configuration and override
(P1) `permissions.auto_mode` SHALL accept `rules.always_block` and `rules.always_allow` (ordered `{ action, resource }` patterns applied before the classifier), `policy` (text appended to the classifier prompt describing the user's boundaries), `classify_read_only` (default false: read-only tools skip the classifier) and `fallback` (`ask` default, or `deny`). `/approve` SHALL re-issue the most recent call blocked by the classifier once, after the user confirms it in a permission prompt showing the classifier's reason; `cyber permissions auto show|reset` SHALL print or clear learned per-checkout statistics. Org policy MAY set the same keys as ceilings (`always_block` unions, `always_allow` intersects).

#### Scenario: Always block a deploy command
- **WHEN** `permissions.auto_mode.rules.always_block` contains `{ action: "bash", resource: "kubectl apply *" }`
- **THEN** that command is blocked in `auto` without consulting the classifier

#### Scenario: Override one block
- **WHEN** the classifier blocked `git push origin feature/x` and the user runs `/approve`
- **THEN** the user sees the command and the classifier's reason, and on confirmation the push runs once

### Requirement: Rule dry run
(P1) `cyber permissions test [--agent <a>] [--mode <m>] (--tool <name> --resource <r> | -- <shell command>)` SHALL evaluate the effective ruleset exactly as a Session would (including bash command splitting, protected paths, saved approvals, Session Mode and org policy) and print the resulting effect, the matching rule and its source layer for each evaluated resource, without creating a Session or running anything. `GET /api/v1/permissions/test` SHALL expose the same evaluation.

#### Scenario: Explain why a command would prompt
- **WHEN** the user runs `cyber permissions test -- "npm test && git push"`
- **THEN** the output lists `npm test → allow (saved approval, project)` and `git push → ask (default rules)`

### Requirement: Additional working directories
(P0) `--add-dir <path>` (repeatable), `/add-dir <path>` and `POST /api/v1/sessions/:id/directories` SHALL add a directory to the Session: it SHALL be allowed for `external_directory` reads and edits under ordinary tool rules, added to the sandbox writable roots, and its instruction files loaded as a Context Source. Additions SHALL be recorded as durable `session.directory.added.1` events, fire the `DirectoryAdded` hook, and apply at the next Safe Boundary. Protected paths inside an added directory stay protected.

#### Scenario: Shared library next to the repo
- **WHEN** the user runs `cyber --add-dir ../shared-lib`
- **THEN** reads and edits under `../shared-lib` need no external-directory prompt and its `AGENTS.md` is in the context
