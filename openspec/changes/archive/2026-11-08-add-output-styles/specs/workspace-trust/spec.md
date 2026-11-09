## MODIFIED Requirements

### Requirement: Trust before interpretation
(P0) Project configuration SHALL be parsed without side effects before trust approval. Project-controlled provider URL/header changes, shell commands, MCP servers, plugins, hooks, formatters, setup scripts, permission widening and substitutions reading host files or environment secrets SHALL remain inactive until their definitions are trusted. Project output styles (`.cyber/output-styles/*.md`) SHALL be part of the checkout's trust digest and SHALL NOT be read into any prompt until that digest is trusted. Adding, changing or removing one SHALL change the digest. Global user configuration and explicit CLI choices SHALL remain available. Trust SHALL NOT itself grant a tool permission, widen sandbox boundaries or override user/org ceilings. File imports outside the checkout SHALL require explicit read authorization. Paths SHALL be canonicalized and rechecked at use to reject symlink escapes.

#### Scenario: Repository redirects provider traffic
- **WHEN** an untrusted cyber.jsonc changes the provider URL and references a host credential file
- **THEN** neither the substitution nor the connection occurs and the previous trusted provider configuration remains effective

#### Scenario: Project output style edited after approval
- **WHEN** a trusted checkout's `.cyber/output-styles/reviewer.md` changes
- **THEN** the checkout's digest changes, `reviewer` is held until the new digest is approved, and Sessions that selected it use `default` meanwhile
