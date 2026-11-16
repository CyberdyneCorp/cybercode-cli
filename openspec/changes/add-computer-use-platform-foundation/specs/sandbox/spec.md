## ADDED Requirements

### Requirement: Sandboxed processes hold no native computer authority
(P2) On every platform, sandboxed tool and plugin processes SHALL NOT receive native capture or input handles, platform capabilities that grant screen capture or input injection, or read access to private computer captures and owner state. Platform changes SHALL specify the mechanism (for example masking a desktop bus or restricting a container capability list). The sandbox SHALL NOT be described as containing full-access owner processes or native GUI applications.

#### Scenario: Sandboxed child reads another Session's capture
- **GIVEN** a Session owns a private computer capture
- **WHEN** a sandboxed command in another Session tries to read the capture directory
- **THEN** access is denied while the owner frame API still serves the capture
