## ADDED Requirements

### Requirement: Supported optional browser integration
(P1) The system SHALL offer a version-pinned browser MCP integration with documented install, health and cleanup commands. It SHALL support navigation, DOM inspection, input, screenshots, console errors and failed network requests. Browsers SHALL use fresh per-Session profiles, never the user default profile or cookies. Installation and project-defined browser servers SHALL follow workspace-trust; browser actions SHALL pass tool permissions and network policy. Browser support SHALL remain optional for non-web projects.

#### Scenario: Verify local app
- **WHEN** a trusted browser integration opens an approved local development URL
- **THEN** the agent can inspect and interact with the page using an isolated profile and capture console errors

### Requirement: Verification artifacts and lifecycle
(P1) Browser evidence SHALL record Session ID, workspace content hash, URL, viewport, timestamp, action summary and screenshot or console artifact hashes. Reports SHALL distinguish observed behavior from assertions not run and mark evidence stale after relevant code changes. Dev servers SHALL be owned background jobs with explicit readiness checks and timeouts; Session cleanup SHALL close the browser and owned jobs. Artifacts SHALL obey redaction, retention and export rules and SHALL NOT be uploaded automatically.

#### Scenario: App startup fails
- **WHEN** the development server exits before its readiness check passes
- **THEN** verification records startup failure with logs and does not claim that the page was tested
