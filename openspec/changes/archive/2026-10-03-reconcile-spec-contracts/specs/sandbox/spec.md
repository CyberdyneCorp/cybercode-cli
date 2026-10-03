## MODIFIED Requirements

### Requirement: Writable roots
(P0) Under `workspace-write`, the writable roots SHALL be: the Location directory, its git worktree root, the session worktree when present, a private per-Session temporary directory (also used as TMPDIR), the managed tool-output and jobs directories, and the entries in `sandbox.writable_roots` (with `~` expanded). Inside the roots, `.git/` (except the working tree), `.git/hooks`, and the protected configuration documents listed by `permissions-modes` (`cyber.json(c)`, `.cyber/cyber.jsonc`, `.cyber/cyber.local.jsonc`, `.cyber/hooks.jsonc`, `.cyber/mcp.json`, `.cyber/plugins*.json`, `.cyber/plugins.lock`, `.cyber/plugins/`) SHALL remain read-only. The `.cyber/` content directories (`plans/`, `workflows/`, `agents/`, `skills/`, `commands/`, `teams/`, `output-styles/`) SHALL be writable.

#### Scenario: Git hooks stay read-only
- **WHEN** a sandboxed command writes `.git/hooks/pre-commit` inside the Location
- **THEN** the write is denied

#### Scenario: Plan directory writable
- **WHEN** a sandboxed `plan`-mode write targets `.cyber/plans/auth.md`
- **THEN** the OS sandbox permits the write and the permission engine evaluates it under plan-mode rules

#### Scenario: Hook config read-only
- **WHEN** a sandboxed command writes `.cyber/hooks.jsonc`
- **THEN** the write is denied by the sandbox
