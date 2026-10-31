## ADDED Requirements

### Requirement: Redacted secret guidance
(P1) The base prompt SHALL explain that secrets in tool output appear as `[REDACTED:<kind>]`: the value exists but is hidden. It SHALL tell the model never to write a marker into a file or a command, to refer to the secret indirectly (the environment variable or file that holds it), and to leave the lines that hold it unchanged.

#### Scenario: Prompt names the marker
- **WHEN** a Session starts with any model family
- **THEN** its base prompt contains `[REDACTED:<kind>]` and the instruction not to write markers
