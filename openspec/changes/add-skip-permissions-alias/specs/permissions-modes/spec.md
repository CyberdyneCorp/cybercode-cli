## MODIFIED Requirements

### Requirement: bypass mode
(P1) `bypass` SHALL evaluate every request as `allow` except explicit user/global deny ceilings, protected paths, critical-path removals, workspace trust and org policy denies. It SHALL be enabled only when the process runs inside the OS sandbox with `workspace-write` or stricter, or inside a detected container or VM, or when `--dangerously-bypass-permissions` is passed explicitly. `--dangerously-skip-permissions` SHALL count as that explicit flag where the CLI accepts it (`cyber exec --embedded|--ephemeral`, an embedded TUI) and SHALL also select `bypass`; it SHALL NOT override org policy `modes.disable`. Enabling it outside these conditions SHALL fail with `bypass mode requires a sandbox, a container, or --dangerously-bypass-permissions`.

#### Scenario: Bypass refused on bare host
- **WHEN** the user sets `mode: "bypass"` with sandbox `full-access` on a host machine and no flag
- **THEN** the Session refuses to start in bypass and reports the requirement

#### Scenario: Skip flag does not override policy
- **WHEN** org policy lists `bypass` in `modes.disable` and a user runs `cyber exec --embedded --dangerously-skip-permissions "deploy"`
- **THEN** the command prints `bypass mode is disabled by your organization` and fails
