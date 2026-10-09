## MODIFIED Requirements

### Requirement: Project MCP file compatibility
(P0) The system SHALL also read `.mcp.json` (Claude Code format, `mcpServers` map) and `.cyber/mcp.json` at the project root, merging them below `cyber.jsonc` entries of the same name. Project-defined servers SHALL require user trust (recorded per server definition hash) before first start.

#### Scenario: Untrusted project server
- **WHEN** a cloned repo contains `.mcp.json` defining server `db`
- **THEN** `db` is not started and the user is asked once to trust it

#### Scenario: Effective server approval is independent and uncached
- **WHEN** a project-controlled server is selected for launch
- **THEN** current checkout approval and separate approval of the effective server name and definition SHALL both be required
- **AND** workspace or hook approval SHALL NOT grant individual server approval
- **AND** changed effective fields or revocation SHALL refuse reuse

#### Scenario: A project field overrides a global server
- **WHEN** a project layer overrides arguments or another field of a globally defined server
- **THEN** every contributing origin SHALL be checked and the effective definition SHALL require project server approval
- **AND** missing or unsupported provenance and malformed trust storage SHALL refuse authorization

#### Scenario: Selected profile retains MCP ownership
- **WHEN** a selected profile combines global MCP fields with project-controlled arguments
- **THEN** effective field provenance SHALL preserve the original global/project origins, including escaped profile names
- **AND** selecting the profile SHALL NOT remove the requirement for project server approval or change the configured sandbox scope

#### Scenario: Review and approve an exact named server
- **WHEN** the user reviews `cyber mcp definitions` and approves `cyber mcp trust NAME --digest DIGEST`
- **THEN** review SHALL preserve original field origins while redacting environment/header values, OAuth settings and URL queries without changing the effective digest
- **AND** approval SHALL require the exact currently loaded project server definition and current checkout approval
- **AND** review/approval SHALL NOT start servers or create Sessions

#### Scenario: Revoke an obsolete server approval
- **WHEN** the current configuration is invalid and the user runs `cyber mcp untrust --digest DIGEST`
- **THEN** the individual approval SHALL remain revocable without interpreting the invalid configuration
