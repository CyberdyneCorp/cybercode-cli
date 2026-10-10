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

### Requirement: Imported edit-tool permission granularity
(P1) Ordered permission rules SHALL support an optional exact native `tool` selector in addition to action/resource/effect. Rules without this selector SHALL preserve their existing meaning. Claude Write and NotebookEdit selectors SHALL map to the shared edit action scoped to write and notebook_edit respectively; Claude Edit/MultiEdit SHALL map to edit scoped to edit. It SHALL NOT infer an apply_patch grant: patch operations can create/remove files and are not the same source tool. The host SHALL bind the actual invocation identity before permission, protected-path, ancestor-mode or auto-review decisions. A scoped allow SHALL NOT match another tool or an evaluation without a tool identity. Scoped deny ceilings SHALL survive bypass, saved approvals and later lower-priority grants. Model tool catalogs SHALL hide fully denied scoped tools without hiding unrelated edit tools. Invalid tool selector types or empty/control-containing selectors SHALL fail closed rather than become unscoped allows.

#### Scenario: Write allow does not grant all edits
- **WHEN** imported permissions allow Write but do not allow Edit
- **THEN** an ordinary write SHALL be allowed under that scoped rule
- **AND** an edit or patch SHALL retain its own permission decision

#### Scenario: Scoped denial survives bypass
- **WHEN** imported permissions deny Write for a path while allowing Edit
- **THEN** write SHALL be denied in default and bypass modes
- **AND** edit SHALL remain independently governed
