## MODIFIED Requirements

### Requirement: Windows enforcement
(P1) On Windows the system SHALL run sandboxed processes with a restricted token in an AppContainer, with ACLs granting write access to writable roots only. When unavailable, it SHALL report `sandbox unavailable` as on Linux.

#### Scenario: Windows restricted write
- **WHEN** a sandboxed PowerShell command writes `C:\Windows\Temp\x` outside the writable roots
- **THEN** access is denied

#### Scenario: Windows command process-tree lifetime
- **WHEN** a native Windows command leaves descendants running and its command owner exits normally or is terminated
- **THEN** the descendants SHALL terminate with the owner
- **AND** process-tree setup failure SHALL refuse to launch the command
- **AND** successful command completion SHALL preserve its exit code

### Requirement: Network isolation and allowlist proxy
(P0) Sandboxed processes SHALL have no direct network access by default (`sandbox.network: "proxy"`). Their HTTP(S) traffic SHALL go through a local proxy that allows only domains in `sandbox.allowed_domains`, which defaults to the package registries `registry.npmjs.org`, `pypi.org`, `files.pythonhosted.org`, `crates.io`, `static.crates.io`, `proxy.golang.org` and `github.com`. A connection to another domain SHALL raise a `network` permission request with the domain as resource, and an approval SHALL add the domain for the Session (or persist it with `always`). `sandbox.network: "off"` SHALL block everything, and `"on"` SHALL allow everything.

#### Scenario: Unknown domain prompts
- **WHEN** a sandboxed `curl https://example.org` runs with the default allowlist
- **THEN** a `network` request for `example.org` is raised and the connection waits up to 120 s for a decision
