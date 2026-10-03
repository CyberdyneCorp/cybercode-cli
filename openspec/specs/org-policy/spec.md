# org-policy Specification

## Purpose
Org policy lets administrators enforce settings that users and projects cannot widen. It covers which providers and models are allowed, which MCP servers, hooks and plugins may run, permission-mode ceilings, sandbox enforcement, telemetry and spend limits. Policy comes from OS-managed files (as in OpenCode v1's managed config and Claude Code's managed settings) and from server-managed policy fetched for the active Cyber Account org, keyed by the CyberdyneAuth `orgs` claim. OpenCode v2's `policies` allow/deny vocabulary is generalized here into one evaluated policy document.

## Requirements

### Requirement: Policy sources
(P3) The system SHALL load policy from the sources below. Within each source, `cyber-policy.json` and `cyber-policy.jsonc` SHALL be read:
1. the OS-managed directory: `/etc/cyber/` on Linux, `/Library/Application Support/cyber/` on macOS, `%ProgramData%\cyber\` on Windows
2. on macOS, the managed preferences domain `dev.cyber-code.managed`, converted from plist
3. on Windows, the registry key `HKLM\Software\Policies\CyberCode`
4. server-managed policy for the active org (see the fetch requirement)

Org-policy files SHALL NOT be writable by the user. A policy file owned by the current non-root user SHALL be ignored with a warning.

#### Scenario: User-owned policy file ignored
- **WHEN** `/etc/cyber/cyber-policy.json` is owned by uid 1000 and the user is uid 1000
- **THEN** the file is ignored and `cyber doctor` reports `managed policy file not root-owned; ignored`

### Requirement: Server-managed policy fetch
(P3) When a Cyber Account is logged in and an active org is selected from `orgs`, the system SHALL fetch `GET {cloud}/api/v1/orgs/{org_id}/policy` with the access token. The response SHALL be cached in `<state>/policy-<org_id>.json` with its `etag`, revalidated every 15 minutes and at server start. When the fetch fails, the cached policy SHALL be used for up to `policy.offline_grace_hours` (default 72). After that, if `fail_closed: true` is in the cached policy, account-gated and network features SHALL be disabled until the next successful fetch.

#### Scenario: Offline within grace
- **WHEN** the policy fetch fails for 10 hours with a valid cache
- **THEN** the cached policy continues to apply and no feature is disabled

#### Scenario: Fail closed after grace
- **WHEN** the cached policy has `fail_closed: true` and fetches have failed for 80 hours
- **THEN** remote control, cross-machine messaging and hosted runners are disabled with `PolicyUnavailableError`, while local sessions continue under the cached restrictions

### Requirement: Precedence and merge
(P3) Policy layers SHALL apply after all user and project config layers. Server-managed policy SHALL override OS-managed policy for the same key, except where an OS-managed key is marked `"lock": "device"`. Policy values SHALL replace config values at the same path. Allow-lists from multiple policy layers SHALL intersect, and deny-lists SHALL union.

#### Scenario: Intersecting allow-lists
- **WHEN** OS policy allows providers `["openai","anthropic","ollama"]` and org policy allows `["anthropic","ollama"]`
- **THEN** only `anthropic` and `ollama` are usable

### Requirement: Locked keys cannot be widened
(P3) Any config path set by policy SHALL be locked. Attempts to change it through config files, profiles, CLI flags, environment variables, `PATCH /api/v1/config` or the TUI SHALL be ignored with a `PolicyLockedError` diagnostic that names the policy source. Narrowing (a stricter value) SHALL be allowed only for keys policy declares as ceilings.

#### Scenario: Flag cannot bypass a locked mode
- **WHEN** policy sets `modes.max = "accept-edits"` and the user runs `cyber --mode bypass`
- **THEN** the session starts in `accept-edits` and stderr shows `mode "bypass" exceeds org ceiling "accept-edits" (policy: org acme)`

### Requirement: Provider and model allow-lists
(P3) Policy SHALL support `providers.allow`, `providers.deny`, `models.allow` and `models.deny`, with wildcard patterns over `provider/model` refs. Denied providers and models SHALL be removed from the catalog before any consumer sees it, and a Session whose model becomes denied SHALL fail its next Turn with `ModelDeniedByPolicyError`.

#### Scenario: Model wildcard deny
- **WHEN** policy sets `models.deny = ["*/*-preview*"]`
- **THEN** `cyber models` does not list preview models, and selecting one fails

### Requirement: Permission mode ceiling
(P3) Policy SHALL support `modes.max`, ordered from least to most permissive: `plan` < `default` < `accept-edits` < `dont-ask` < `auto` < `bypass`. Also `modes.disable` (a list) and `modes.default`. A requested mode above the ceiling SHALL be clamped to the ceiling. `bypass` SHALL be disabled by default in any policy unless explicitly allowed.

#### Scenario: Auto mode disabled
- **WHEN** policy sets `modes.disable = ["auto"]`
- **THEN** the mode picker omits `auto`, and `--mode auto` falls back to `default` with a warning

### Requirement: Permission rules enforced by policy
(P3) Policy SHALL support `permissions.deny` and `permissions.ask` rule arrays with the same rule syntax as user permissions. These rules SHALL be evaluated after all user, project, agent and session rules, so a policy `deny` is final and a policy `ask` cannot be downgraded to `allow` by saved approvals.

#### Scenario: Policy deny beats a saved approval
- **WHEN** the user previously saved "always allow `bash: curl *`" and policy denies `bash: curl *`
- **THEN** `curl https://x` is blocked with `denied by org policy`

### Requirement: Sandbox enforcement
(P3) Policy SHALL support `sandbox.required = true`, `sandbox.network.allow_domains`, `sandbox.filesystem.deny_read` and `sandbox.allow_unsandboxed_commands = false`. When the sandbox is required but unavailable on the host, tool execution of `bash` SHALL be refused with `SandboxRequiredError`, rather than running unsandboxed.

#### Scenario: Required sandbox missing
- **WHEN** policy requires the sandbox and `bwrap` is not installed on Linux
- **THEN** every bash call fails with `SandboxRequiredError: org policy requires the sandbox; install bubblewrap`

### Requirement: Extension allow-lists
(P3) Policy SHALL support `mcp.allow`/`mcp.deny` (by server name, command or URL pattern), `plugins.allow`/`plugins.deny` (by package name and optional version range), `hooks.allow_user_hooks` (default true), `channels.allow` and `marketplaces.allow`. Blocked extensions SHALL NOT be started, and SHALL be listed by `cyber doctor` with the blocking source. Policy MAY also define `mcp.managed` servers that are always provided.

#### Scenario: User hooks disabled
- **WHEN** policy sets `hooks.allow_user_hooks = false`
- **THEN** only hooks defined in policy run, and project and user hook files are ignored with a diagnostic

### Requirement: Feature switches
(P3) Policy SHALL support boolean switches:
- `features.remote_control`
- `features.cross_machine_messaging`
- `features.hosted_runners`
- `features.self_hosted_runners_only`
- `features.sharing` (`off`, `org`, `public`)
- `features.routines`
- `features.workflows`
- `features.memory`
- `features.web_tools`
- `features.auto_update`

Disabled features SHALL be hidden from the TUI and fail with `FeatureDisabledByPolicyError` when invoked.

#### Scenario: Public sharing disabled
- **WHEN** policy sets `features.sharing = "org"`
- **THEN** `/share` creates links visible only to members of the active org, and `--public` fails

### Requirement: Spend limits
(P3) Policy SHALL support `spend.limit_usd` per `day`, `week` or `month`, per user and per project. Spend SHALL be computed from recorded usage costs across all local Sessions, Workflow Runs and Loops. At 80% of a limit the user SHALL see a warning. At 100%, new Turns SHALL be refused with `SpendLimitReachedError`, except for models with zero configured cost.

#### Scenario: Daily limit reached
- **WHEN** policy sets `spend.limit_usd.day = 20` and today's recorded cost reaches $20.00
- **THEN** the next Turn fails with `SpendLimitReachedError` and the Drain stops; local zero-cost models remain usable

### Requirement: Telemetry policy
(P3) Policy SHALL be able to force telemetry `off`, or force `on` with an OpenTelemetry endpoint and headers, and to require prompt content redaction. A user setting SHALL NOT enable content capture when policy requires redaction.

#### Scenario: Forced OTLP export
- **WHEN** policy sets `telemetry.otlp.endpoint = "https://otel.acme.com"` and `telemetry.redact_content = true`
- **THEN** metrics and traces go to that endpoint, and prompt and response bodies are never included

### Requirement: Policy inspection
(P3) `cyber debug policy` and `GET /api/v1/policy` SHALL show the effective policy, each value's source (`os:/etc/cyber/cyber-policy.json`, `org:acme@etag`), and the user settings it overrides.

#### Scenario: Explaining a locked value
- **WHEN** the user runs `cyber debug policy`
- **THEN** the output lists `modes.max = accept-edits (org:acme)` and `overrides: project cyber.jsonc mode=bypass`

### Requirement: Policy change application
(P3) When the effective policy changes, the system SHALL publish `policy.updated.1`, rebuild the affected domains, apply new restrictions at each Session's next Safe Boundary, and stop processes for extensions that are now blocked (MCP servers, plugins, channels) within 10 seconds.

#### Scenario: MCP server blocked mid-session
- **WHEN** an org admin adds `mcp.deny = ["notion"]` while a session uses the `notion` server
- **THEN** the server is stopped within 10 seconds, and its tools are absent from the next Turn
