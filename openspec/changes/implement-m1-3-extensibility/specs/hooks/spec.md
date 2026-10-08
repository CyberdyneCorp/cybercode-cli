## MODIFIED Requirements

### Requirement: Hook configuration
(P1) The system SHALL read hooks from the `hooks` config key as a map from event name to an ordered array of hook groups `{ matcher?, hooks: [handler, ...] }`, where each handler has a required `type` (`command`, `http`, `prompt`, or `mcp_tool`) and optional `timeout` (seconds), `async` (boolean), `id` and `description`. Unknown event names SHALL fail config validation with the path of the offending key.

#### Scenario: Valid PreToolUse hook
- **WHEN** `cyber.jsonc` contains a `PreToolUse` group with matcher `bash` and a command handler
- **THEN** the definition SHALL resolve with its declared matcher and command

#### Scenario: Unknown event rejected
- **WHEN** a configuration declares `hooks.BeforeEverything`
- **THEN** configuration loading SHALL fail with an invalid-config error naming that key

#### Scenario: Invalid handler contract rejected
- **WHEN** a trusted hook has an unsupported type, a missing type-specific field, an invalid selector or a timeout outside 1 through 600 seconds
- **THEN** configuration loading SHALL fail with the hook definition path
- **AND** validation SHALL execute no handlers

### Requirement: Hook scopes and merge order
(P1) The system SHALL collect hooks from the managed, global (`~/.config/cyber`), project (`cyber.jsonc` and `.cyber/` files) and local (`.cyber/cyber.local.jsonc`) scopes plus enabled plugins, and SHALL run every matching hook from every scope rather than letting one scope replace another. Hooks SHALL execute in the order managed, global, project, local, plugin. When `policy.hooks.managed_only` is true, only managed-scope hooks SHALL run.

#### Scenario: Hooks from two scopes both run
- **WHEN** global and project scopes each define a `PostToolUse` hook matching `edit`
- **THEN** both contributions SHALL survive configuration resolution and run in scope order

#### Scenario: Per-handler origins remain available
- **WHEN** hook groups from different configuration files are appended
- **THEN** every group and handler SHALL retain its own source label at its resolved index
- **AND** an empty later event array SHALL NOT erase earlier groups

#### Scenario: Managed-only policy
- **WHEN** the organization policy sets `hooks.managed_only: true`
- **THEN** project, local, global and plugin hooks SHALL be skipped and a single notice SHALL list how many were skipped

#### Scenario: Selected profile preserves hook origins
- **WHEN** global, project and local files contribute hooks to the same selected profile
- **THEN** all contributions SHALL be appended to ordinary hook definitions
- **AND** every selected group, handler and handler field SHALL retain its defining file origin rather than a generic profile label
- **AND** untrusted or changed project definitions SHALL remain withheld and an empty later profile event array SHALL NOT erase earlier definitions

### Requirement: Trust for project hooks
(P1) The system SHALL require explicit user trust before running project-scope or local-scope hooks. Trust SHALL use the checkout-scoped workspace-trust store and the SHA-256 of each handler definition. A new or changed handler SHALL be skipped and reported as `untrusted` until approved via `/hooks` or `cyber hooks trust`. In `exec` mode, untrusted hooks SHALL be skipped unless `--trust-project-hooks` explicitly approves the currently inspected handler digests for that invocation, without approving future changes.

#### Scenario: Changed hook requires re-trust
- **WHEN** a teammate changes `.cyber/cyber.jsonc` hook command after the user trusted it
- **THEN** the hook is skipped and the TUI shows `1 untrusted hook changed — review with /hooks`

#### Scenario: Workspace approval does not approve handlers
- **WHEN** a checkout's configuration is approved but its hook handler digest is not
- **THEN** the shared trust store SHALL report that handler as unapproved
- **AND** adding a handler approval SHALL preserve the workspace approval and other approved handler digests

#### Scenario: Invocation approval remains temporary
- **WHEN** an invocation approves inspected handler digests
- **THEN** only those digests in that canonical checkout SHALL be approved for that invocation
- **AND** changed definitions and other checkouts SHALL remain unapproved
- **AND** no durable handler approval SHALL be written

#### Scenario: Shared trust storage is serialized
- **WHEN** multiple processes update workspace and handler approvals concurrently
- **THEN** all independent approvals SHALL survive in the private shared store
- **AND** legacy workspace-only files SHALL remain readable
- **AND** checkout revocation SHALL remove that checkout's workspace and handler approvals

### Requirement: Hooks viewer and CLI
(P1) The system SHALL provide `/hooks` in the TUI and `cyber hooks list|trust|untrust|test <event>` on the CLI. These SHALL show every hook with its scope, event, matcher, type, trust state, last run time and last result. `cyber hooks test` SHALL run matching hooks against a synthetic or `--payload <file>` event and print their decisions without affecting any session.

#### Scenario: Dry-run a hook
- **WHEN** the user runs `cyber hooks test PreToolUse --payload ev.json`
- **THEN** each matching handler runs and its parsed decision is printed

#### Scenario: Inspect and approve a resolved handler
- **WHEN** a checkout's configuration is trusted and the user lists hooks
- **THEN** resolved handlers SHALL show source, scope, definition digest and individual trust state with credentials redacted
- **AND** trust SHALL approve only a currently resolved project/local definition digest
- **AND** a changed definition SHALL remain unapproved

#### Scenario: Revoke an obsolete handler
- **WHEN** the user untrusts a previously approved handler digest after the definition is removed or current configuration becomes malformed
- **THEN** the approval SHALL be revoked without resolving executable configuration

### Requirement: Matchers
(P1) The system SHALL match hook groups by `matcher`, which is either a glob over the event's subject (tool name for tool events, including `mcp__<server>__<tool>`; notification type; file path for `FileChanged`) or, when wrapped in `/.../`, a regular expression. An absent or `*` matcher SHALL match every subject. A group MAY add `paths` (globs relative to the Location) that SHALL also match for tool calls with file targets.

#### Scenario: Regex matcher for MCP tools
- **WHEN** a hook group has matcher `/^mcp__github__.*/`
- **THEN** it SHALL match `mcp__github__create_issue` and not `bash`

#### Scenario: Path filter
- **WHEN** a `PreToolUse` group has matcher `edit` and `paths: ["migrations/**"]`
- **THEN** it SHALL match edits under `migrations/` only
- **AND** absolute or escaping path candidates SHALL NOT satisfy a Location-relative path filter

### Requirement: Decision schema
(P1) A hook decision SHALL be a JSON object with optional fields: `decision` (`allow`, `deny`, `ask`), `reason`, `updated_input` (PreToolUse only, replaces tool input after re-validation against the tool schema), `additional_context` (text admitted as a system message at the next Safe Boundary; for `PreCompact` it is appended to the summary instructions instead), `continue` (false stops the Drain after the current Turn), `stop_reason`, and `suppress_output` (hide the hook's output from the transcript). Fields not valid for the event SHALL be ignored with a debug log. The explicit Stop `block` and PermissionDenied `retry`/`updated_input` contracts SHALL remain valid for those events.

#### Scenario: Input rewritten
- **WHEN** a `PreToolUse` hook returns `{"updated_input": {"command": "npm test -- --ci"}}` for a bash call
- **THEN** the bash tool SHALL run `npm test -- --ci` and the transcript SHALL show the rewrite

#### Scenario: Invalid rewritten input
- **WHEN** `updated_input` fails the tool's input schema
- **THEN** the call SHALL be denied with `hook produced invalid tool input`

#### Scenario: Event-specific fields
- **WHEN** a PostToolUse decision contains updated_input or a non-Stop decision contains block
- **THEN** those fields SHALL be ignored and identified for debug diagnostics
- **AND** malformed applicable decision fields SHALL fail decision validation

### Requirement: Decision merging
(P1) When several hooks return decisions for one event, the system SHALL apply them in execution order and combine them: any `deny` SHALL win over `ask`, and `ask` SHALL win over `allow`. `updated_input` SHALL chain, with each later hook receiving the previous hook's output. `additional_context` values SHALL be concatenated. Any `continue: false` SHALL stop continuation.

#### Scenario: Deny beats allow
- **WHEN** one hook returns `allow` and a later hook returns `deny`
- **THEN** the action SHALL be denied with the later hook's reason
- **AND** another later allow SHALL NOT erase that denial

#### Scenario: Ordered context and continuation
- **WHEN** successive hooks add context, rewrite input and set continue false
- **THEN** context SHALL retain declared order and the next hook SHALL receive the rewritten input
- **AND** a later continue true SHALL NOT erase the stop request
