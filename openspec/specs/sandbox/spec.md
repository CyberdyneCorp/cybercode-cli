# sandbox Specification

## Purpose
Confines the processes Cyber Code launches for the model (shell commands, optionally MCP stdio servers, hooks and formatters) with operating-system enforcement of filesystem and network boundaries. Permission prompts then become a second line of defense instead of the only one. Draws on Codex (Seatbelt/Landlock policies, escalation requests) and Claude Code (sandboxed Bash with a network allowlist proxy and credential masking). OpenCode has no equivalent.

## Requirements

### Requirement: Sandbox policies
(P0) The system SHALL support sandbox policies `read-only`, `workspace-write` (default) and `full-access`, configured by `sandbox.policy`, the `--sandbox` flag, or per agent. `read-only` SHALL allow reads anywhere readable and no writes. `workspace-write` SHALL allow writes only to writable roots. `full-access` SHALL disable OS enforcement.

#### Scenario: Default policy blocks writes outside the project
- **WHEN** a sandboxed `bash` command under `workspace-write` writes to `~/notes.txt`
- **THEN** the write fails with `Operation not permitted` and the result says the sandbox blocked it

### Requirement: Writable roots
(P0) Under `workspace-write`, the writable roots SHALL be: the Location directory, its git worktree root, the session worktree when present, a private per-Session temporary directory (also used as TMPDIR), and the entries in `sandbox.writable_roots` (with `~` expanded). `.git/hooks`, `.cyber/` and the protected paths of `permissions-modes` SHALL remain read-only even inside the roots.

#### Scenario: Git hooks stay read-only
- **WHEN** a sandboxed command writes `.git/hooks/pre-commit` inside the Location
- **THEN** the write is denied

### Requirement: macOS enforcement
(P0) On macOS the system SHALL launch sandboxed processes under a generated Seatbelt profile (`sandbox-exec` semantics via the private API or the `sandbox-exec` binary) that encodes the policy's read, write and network rules. Child processes SHALL inherit the profile.

#### Scenario: Child process inherits profile
- **WHEN** a sandboxed `make` spawns `cc` that tries to write `/usr/local/lib`
- **THEN** the write is denied

### Requirement: Linux enforcement
(P0) On Linux (including WSL2) the system SHALL enforce filesystem rules with Landlock (ABI ≥ 1) and restrict syscalls with seccomp, running network isolation in a separate network namespace. When Landlock is unavailable it SHALL fall back to `bubblewrap` if installed. If neither is available, `cyber doctor` and the session header SHALL report `sandbox unavailable`, and execution requiring sandbox enforcement SHALL fail closed with `SandboxUnavailableError`. Full access SHALL require an explicit user CLI selection and SHALL never be selected by project config or automatic fallback.

#### Scenario: No sandbox support
- **WHEN** the kernel lacks Landlock and bubblewrap is not installed
- **THEN** the Session shows `sandbox unavailable` and refuses sandbox-dependent execution until the user explicitly chooses a supported environment or full access

### Requirement: Windows enforcement
(P1) On Windows the system SHALL run sandboxed processes with a restricted token in an AppContainer, with ACLs granting write access to writable roots only. When unavailable, it SHALL report `sandbox unavailable` as on Linux.

#### Scenario: Windows restricted write
- **WHEN** a sandboxed PowerShell command writes `C:\Windows\Temp\x` outside the writable roots
- **THEN** access is denied

### Requirement: Network isolation and allowlist proxy
(P0) Sandboxed processes SHALL have no direct network access by default (`sandbox.network: "proxy"`). Their HTTP(S) traffic SHALL go through a local proxy that allows only domains in `sandbox.allowed_domains`, which defaults to the package registries `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org`, `crates.io`, `static.crates.io`, `proxy.golang.org` and `github.com`. A connection to another domain SHALL raise a `network` permission request with the domain as resource, and an approval SHALL add the domain for the Session (or persist it with `always`). `sandbox.network: "off"` SHALL block everything, and `"on"` SHALL allow everything.

#### Scenario: Unknown domain prompts
- **WHEN** a sandboxed `curl https://example.org` runs with the default allowlist
- **THEN** a `network` request for `example.org` is raised and the connection waits up to 120 s for a decision

### Requirement: Credential masking
(P0) Sandboxed processes SHALL receive an environment filtered by `sandbox.env`. Variables matching `*_TOKEN`, `*_KEY`, `*_SECRET`, `*PASSWORD*`, `AWS_*`, `GITHUB_TOKEN` and the provider credential variables known to `provider-credentials` SHALL be removed unless listed in `sandbox.env.allow`. Credential files (`~/.aws`, `~/.config/gh`, `~/.netrc`, `~/.docker/config.json`, `~/.ssh`) SHALL be unreadable unless listed in `sandbox.readable_paths`.

#### Scenario: API key hidden from shell
- **WHEN** the server environment has `OPENAI_API_KEY` and the model runs `env` in the sandbox
- **THEN** the output does not contain `OPENAI_API_KEY`

### Requirement: Escalation requests
(P1) When a sandboxed command fails with a sandbox denial, the tool result SHALL say so explicitly (`Sandbox denied: <operation> <path|host>`). The model MAY then re-issue the call with `{ sandbox: "escalate", justification }`, which SHALL raise a `sandbox.escalate` permission request showing the command and justification. Approval SHALL run that single call unsandboxed. In `dont-ask` the escalation SHALL be denied. In `auto` the classifier SHALL decide.

#### Scenario: Escalate to install globally
- **WHEN** `npm install -g foo` is denied by the sandbox and the model asks to escalate
- **THEN** the user sees the command with the justification and can approve it once

### Requirement: Excluded commands
(P1) Commands matching `sandbox.excluded_commands` patterns (for example `docker *`, `gh auth *`) SHALL run outside the sandbox and SHALL always evaluate the `bash` permission as at least `ask`, regardless of Mode except `bypass`.

#### Scenario: Docker runs outside sandbox
- **WHEN** `docker ps` matches an excluded pattern
- **THEN** it runs unsandboxed after approval

### Requirement: Sandbox scope
(P0) The sandbox SHALL apply to `bash`, `powershell`, `monitor` commands and background tasks. With `sandbox.apply_to` (default `["tools", "hooks", "formatters"]`; add `"mcp"` to opt in) it SHALL also apply to hook commands, formatters and MCP stdio servers. Cyber's own server process SHALL never be sandboxed by this mechanism.

#### Scenario: MCP server sandboxed on opt-in
- **WHEN** `sandbox.apply_to` includes `mcp`
- **THEN** stdio MCP servers start under the sandbox with the Location as their only writable root

### Requirement: Container detection
(P1) The system SHALL detect execution inside a container or VM (`/.dockerenv`, `/run/.containerenv`, cgroup markers, `CYBER_CONTAINER=1`, devcontainer environment variables). When detected and no OS sandbox is available, it SHALL treat the environment as isolated for the purposes of `bypass` eligibility and report `isolation: container` in the session header.

#### Scenario: Devcontainer allows bypass
- **WHEN** Cyber runs in a devcontainer without Landlock
- **THEN** `bypass` mode is allowed and the header shows `isolation: container`

### Requirement: Sandbox CLI
(P1) The CLI SHALL provide `cyber sandbox test`, which runs a probe suite and prints read/write/network results for the current policy, `cyber sandbox explain [--policy <p>]`, which prints the effective roots, allowed domains and masked variables, and `cyber sandbox run -- <cmd>`, which runs a command under the effective policy.

#### Scenario: Explain effective policy
- **WHEN** the user runs `cyber sandbox explain`
- **THEN** the writable roots, network mode, allowed domains and masked environment variable names are printed

### Requirement: Org enforcement
(P3) When org policy (`org-policy`) sets `sandbox.required: true` or a minimum policy, users SHALL NOT select a weaker policy or disable the sandbox. Attempts SHALL fail with `Sandbox policy is managed by your organization`. Org-managed `allowed_domains` SHALL replace the local ones unless `policy.sandbox.allow_local_domains` is true.

#### Scenario: Org requires sandbox
- **WHEN** org policy requires `workspace-write` and the user passes `--sandbox full-access`
- **THEN** the Session starts with `workspace-write` and shows the managed-policy notice

### Requirement: Sandbox events and audit
(P1) Each sandbox denial SHALL be recorded as a `sandbox.denied.1` event with `{ session_id, call_id, operation, target }`. Each escalation SHALL be recorded as `sandbox.escalated.1`. Both SHALL be exported via `observability-costs` telemetry when enabled.

#### Scenario: Denial recorded
- **WHEN** a sandboxed command is blocked from writing `/etc/hosts`
- **THEN** a `sandbox.denied.1` event with operation `write` and target `/etc/hosts` is stored
