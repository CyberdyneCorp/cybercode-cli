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

### Requirement: Pure OpenCode permission conversion
(P1) Import planning SHALL convert explicit OpenCode permission/permissions strings, ordered action/resource/effect arrays, ordered action/pattern maps and legacy tools booleans into typed native rules without executing code or accessing files. Legacy tool rules SHALL precede explicit permission rules. It SHALL map shell to bash, task/subagent to agent and write/patch to edit. Map and array ordering SHALL survive conversion. A source literal glob ending in space-star SHALL NOT acquire Cyber's additional bare-command match. Ambiguous simultaneous permission and permissions fields, malformed shapes, unknown actions/effects or resource patterns requiring unresolved home expansion SHALL refuse the whole batch with bounded static errors that do not echo source values. This explicit converter SHALL NOT imply complete source discovery, implicit source defaults or full migration acceptance.

#### Scenario: Explicit permission overrides legacy boolean
- **WHEN** legacy tools disable bash and a later permission rule allows one command
- **THEN** other commands SHALL stay denied and that command SHALL be allowed

#### Scenario: Command prefix retains its literal space
- **WHEN** a source rule allows git followed by a space and wildcard
- **THEN** git status SHALL match
- **AND** bare git SHALL retain its separate decision

### Requirement: Pure Codex argv-prefix conversion
(P1) Import planning SHALL convert parsed constant Codex prefix rules to native bash rules. It SHALL support nonempty literal argv-prefix lists and nonempty alternatives per position, default decision allow and allow/prompt/forbidden effects. It SHALL order output allow, ask, deny, preserving source order within each group, so the most restrictive matching decision survives native last-match-wins evaluation. Expansion SHALL be bounded at 4096 rules and one MiB of command text per batch. Native rules SHALL retain an optional argv_prefix and compare literal Bash argv with exact case, preserving quotes/whitespace normalization rather than matching only source text. Unresolved argv SHALL not receive an imported allow; matching ask/deny rules SHALL conservatively retain their refusal/review boundary. Tokens that cannot be safely represented as literal native command patterns, malformed fields and unknown decisions SHALL refuse the entire batch with indexed static errors without echoing source values. Conversion SHALL neither execute source Starlark nor grant sandbox escalation. Starlark parsing, examples/metadata, source discovery and full Codex configuration/profile/sandbox migration SHALL remain separate required work.

#### Scenario: Later allow cannot erase a forbidden prefix
- **WHEN** a parsed batch forbids git push and later allows git generally
- **THEN** native evaluation SHALL deny git push and allow unrelated git commands

#### Scenario: Literal argv cannot become a wildcard grant
- **WHEN** a prefix token contains a native wildcard or whitespace
- **THEN** conversion SHALL refuse the batch instead of widening the command grant

### Requirement: Literal Codex rules source parsing
(P1) Import planning SHALL parse bounded constant prefix_rule calls from Codex rules text without evaluating Starlark or executing commands. It SHALL accept comments, single/double quoted strings, whitespace, trailing commas, literal argv alternatives, decisions, justification and match/not_match examples. It SHALL retain source metadata and validate each example against its source argv prefix, then apply the shared strongest-match converter to the complete batch. Input SHALL be bounded at one MiB, 4096 calls and two list levels. Unknown statements, executable expressions, duplicate/unknown fields, malformed strings and contradictory examples SHALL refuse the whole batch with static indexed/offset errors without echoing source values. Unsupported dynamic Starlark constructs SHALL remain reportable as not imported, without silently dropping their potentially protective rules. Source file discovery, config/profile/sandbox conversion and reviewed import writing SHALL remain required.

#### Scenario: Source examples are checked before import
- **WHEN** a prefix rule declares git push but includes git status in its match examples
- **THEN** no converted batch SHALL be returned
- **AND** the failure SHALL identify the example index without repeating source text

#### Scenario: Source code cannot run during migration
- **WHEN** source text contains load, assignments or function calls instead of constant prefix_rule fields
- **THEN** parsing SHALL refuse without executing that source

### Requirement: Read-only migration file inventory
(P1) Migration discovery SHALL accept explicit canonical project-root, current-directory, home and optional CODEX_HOME inputs and inventory static Claude/Codex/OpenCode project/global source locations without reading configuration contents, executing commands or writing files. It SHALL preserve source tool, global/project/custom-home layer, file kind and path, walk project layers from root to current directory, and order source groups opencode, codex, claude. It SHALL include settings/config/profile files, instructions, MCP files, agents, commands, skills/assets, rules, hooks/policy files and OpenCode manual-port files. Symlinks and inspection failures SHALL be reported explicitly without following linked directory trees. Traversal SHALL be deterministically ordered and bounded at 4096 directory-entry/metadata inspections and 32 tree levels; limit failures SHALL return no incomplete inventory as a complete result. Referenced custom-agent/config paths, parsed item counts/read-time status, sessions/SQLite counts, CLI detection and import review/writes SHALL remain mandatory subsequent delivery.

#### Scenario: Explicit custom Codex home
- **WHEN** an explicit CODEX_HOME differs from the default user directory
- **THEN** discovered sources SHALL retain which home supplied each config/profile/rule
- **AND** discovery SHALL not change process-global environment

#### Scenario: Linked source directory
- **WHEN** an agents or skills source points through a symbolic link
- **THEN** discovery SHALL report that source without scanning its target
- **AND** no files SHALL be written or executed
