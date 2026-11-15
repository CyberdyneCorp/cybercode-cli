## ADDED Requirements

### Requirement: Windows AppContainer excludes computer capabilities
(P2) As the Windows mechanism for "Sandboxed processes hold no native computer authority", the Windows AppContainer capability set SHALL NOT include graphics-capture capabilities, launch verification SHALL keep refusing unexpected capabilities, and the AppContainer SID SHALL have no access to the private computer capture root.

#### Scenario: Capture capability requested for a sandboxed child
- **WHEN** a sandbox launch plan on Windows would add a graphics-capture capability to the AppContainer
- **THEN** launch verification refuses the plan and no child starts
