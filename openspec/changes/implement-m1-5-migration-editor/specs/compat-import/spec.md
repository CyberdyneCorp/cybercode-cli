## ADDED Requirements

### Requirement: Pure Claude permission conversion
(P1) Import planning SHALL convert Claude permissions allow, ask and deny arrays to typed native rules in that group order, preserving order within each group. It SHALL convert the canonical final Bash prefix selector `:*` to ` *`, strip leading project-relative `./` selectors, map known native actions explicitly and map supported defaultMode aliases. Conversion SHALL perform no filesystem or process operations and SHALL NOT grant trust. A malformed or unsupported rule/mode SHALL refuse the batch with a bounded field/index and static reason, without echoing the source value or returning a permissive partial result. This converter alone SHALL NOT constitute complete import command or milestone acceptance.

#### Scenario: Deny wins over broad allow
- **WHEN** Claude permits Bash generally, asks for a specific command and denies that command
- **THEN** converted rules SHALL cause the existing last-match-wins engine to deny the command
- **AND** unrelated permitted commands SHALL remain permitted

#### Scenario: Invalid source selector
- **WHEN** a source permission is malformed or uses an unmapped action
- **THEN** conversion SHALL return no rule batch
- **AND** its error SHALL identify the source field/index without repeating source contents
