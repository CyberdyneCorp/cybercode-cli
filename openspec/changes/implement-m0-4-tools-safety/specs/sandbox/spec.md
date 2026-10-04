## MODIFIED Requirements

### Requirement: Linux enforcement
(P0) On Linux (including WSL2) the system SHALL enforce sandbox policies with `bubblewrap`: the host mounted read-only, writable roots bound writable, protected paths re-bound read-only, credential files hidden, and, unless `sandbox.network` is `"on"`, a private network namespace. In proxy mode the `cyber-sandbox-exec` helper SHALL forward a loopback port inside the namespace to the allowlist proxy's Unix socket. Where a fresh `/proc` cannot be mounted (for example inside containers that mask it), the host `/proc` SHALL be bound read-only instead. A Landlock-based fallback MAY be used when bubblewrap is unavailable only if it enforces the same read-only and unreadable paths. If no enforcement is available, `cyber doctor` and the session header SHALL report `sandbox unavailable`, and execution requiring sandbox enforcement SHALL fail closed with `SandboxUnavailableError`. Full access SHALL require an explicit user CLI selection and SHALL never be selected by project config or automatic fallback.

#### Scenario: No sandbox support
- **WHEN** bubblewrap is not installed and no equivalent fallback is available
- **THEN** the Session shows `sandbox unavailable` and refuses sandbox-dependent execution until the user explicitly chooses a supported environment or full access

#### Scenario: Protected path inside a writable root
- **WHEN** a sandboxed command on Linux writes `.git/config` inside the Location
- **THEN** the write fails because `.git` is bound read-only over the writable root

#### Scenario: Proxy through the namespace bridge
- **WHEN** a sandboxed `curl https://crates.io` runs in proxy mode on Linux
- **THEN** the request reaches the allowlist proxy through `cyber-sandbox-exec` and a direct connection that bypasses the proxy fails

### Requirement: Writable roots
(P0) Under `workspace-write`, the writable roots SHALL be: the Location directory, its git worktree root, the session worktree when present, a private per-Session temporary directory (also used as TMPDIR), the managed tool-output and jobs directories, and the entries in `sandbox.writable_roots` (with `~` expanded). On macOS the per-user temporary directory (`_CS_DARWIN_USER_TEMP_DIR`) SHALL also be writable, because system tools such as `mktemp` use it even when TMPDIR is set. Inside the roots, `.git/` (except the working tree), `.git/hooks`, and the protected configuration documents listed by `permissions-modes` (`cyber.json(c)`, `.cyber/cyber.jsonc`, `.cyber/cyber.local.jsonc`, `.cyber/hooks.jsonc`, `.cyber/mcp.json`, `.cyber/plugins*.json`, `.cyber/plugins.lock`, `.cyber/plugins/`) SHALL remain read-only. The `.cyber/` content directories (`plans/`, `workflows/`, `agents/`, `skills/`, `commands/`, `teams/`, `output-styles/`) SHALL be writable.

#### Scenario: Git hooks stay read-only
- **WHEN** a sandboxed command writes `.git/hooks/pre-commit` inside the Location
- **THEN** the write is denied

#### Scenario: Plan directory writable
- **WHEN** a sandboxed `plan`-mode write targets `.cyber/plans/auth.md`
- **THEN** the OS sandbox permits the write and the permission engine evaluates it under plan-mode rules

#### Scenario: Hook config read-only
- **WHEN** a sandboxed command writes `.cyber/hooks.jsonc`
- **THEN** the write is denied by the sandbox

#### Scenario: Temporary files on macOS
- **WHEN** a sandboxed command on macOS runs `mktemp`
- **THEN** the file is created in the per-user temporary directory
