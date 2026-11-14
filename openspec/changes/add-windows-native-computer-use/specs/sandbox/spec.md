## ADDED Requirements

### Requirement: Sandboxed processes hold no native computer authority
(P2) Sandboxed processes SHALL NOT receive screen-capture or input-injection capabilities on any platform. On Windows, the AppContainer capability set SHALL NOT include graphics-capture capabilities, and launch verification SHALL keep refusing unexpected capabilities. Private computer captures and owner state SHALL stay unreadable from sandboxed processes. The command sandbox SHALL NOT be described as containing the authority of native GUI applications or of full-access owner processes.

#### Scenario: Sandboxed child reads a capture
- **GIVEN** a Session owns a private computer capture on Windows
- **WHEN** a sandboxed command in another Session tries to read the capture directory
- **THEN** access is denied while the owner frame API still serves the capture
