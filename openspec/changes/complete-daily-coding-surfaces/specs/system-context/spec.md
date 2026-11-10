## ADDED Requirements

### Requirement: Scope-aware rule inspection and loading
(P1) Path-scoped rules SHALL be bound to trusted definition identity, project root, successful authorized file operation and Context Epoch. Failed/denied operations SHALL NOT consume once-per-Epoch delivery. Context inspection SHALL report effective sources, origins, token estimates, pending updates and honest utilization through authenticated read-only API/TUI surfaces, using the same source model as request assembly. It SHALL NOT start inference, launch integrations or read an unauthorized source to produce a count. Existing `instructions` array configuration SHALL remain compatible. The additive `instruction_limits` object SHALL provide `max_file_bytes` default 32768 and `max_total_bytes` default 131072; invalid nonpositive or incompatible limits SHALL fail configuration validation before loading sources.

#### Scenario: Rule after a denied read
- **WHEN** a matching file read is denied and a later authorized edit succeeds in the same Epoch
- **THEN** the read does not expose or consume the rule and the successful edit delivers it once

#### Scenario: Inspection of an untrusted source
- **WHEN** an owner inspects context while project sources are withheld by trust policy
- **THEN** inspection reports their withheld state without loading their content or treating their estimated bytes as active context

## MODIFIED Requirements

### Requirement: Path-scoped rules and overrides
(P1) The system SHALL load rule files from `.cyber/rules/*.md` and, unless `compat.claude` is false, `.claude/rules/*.md` in each config directory. A rule file without frontmatter `paths` SHALL join `core/instructions` in the baseline. A rule file with `paths: [globs]` (relative to its project root) SHALL be attached through the nested-instruction mechanism the first time per Context Epoch that a matching file is read, edited or patched, labelled with its path. An `AGENTS.override.md` in a directory SHALL take the `AGENTS.md` position and replace that directory's `AGENTS.md`, `CLAUDE.md` and `CYBER.md` instead of being appended; that directory's `CONTEXT.md` still loads. Each instruction or rule file, including `CYBER.md`, SHALL be truncated at `instruction_limits.max_file_bytes` (default 32768) with a notice, and `cyber doctor` SHALL warn about truncated files. The baseline instruction text as a whole, imports included, SHALL be bounded by `instruction_limits.max_total_bytes` (default 131072 bytes): the file that crosses the bound SHALL be cut with a notice, later files and imports SHALL be left out with a warning.

#### Scenario: Migration rules only for migrations
- **WHEN** `.cyber/rules/migrations.md` has `paths: ["db/migrations/**"]` and the model edits `db/migrations/0042.sql`
- **THEN** the edit result includes the rule file in a `<system-reminder>` block, once for the epoch, and unrelated edits never load it

#### Scenario: Override file replaces
- **WHEN** `/repo/packages/api/` contains both `AGENTS.md` and `AGENTS.override.md`
- **THEN** only `AGENTS.override.md` is loaded for that directory

#### Scenario: Override file replaces CYBER.md
- **WHEN** `/repo/packages/api/` contains `AGENTS.override.md`, `CYBER.md` and `CONTEXT.md`
- **THEN** `AGENTS.override.md` and then `CONTEXT.md` are loaded for that directory, and `CYBER.md` is not

#### Scenario: Oversized CYBER.md
- **WHEN** `/repo/CYBER.md` is larger than `instruction_limits.max_file_bytes`
- **THEN** its content is cut at that size with a truncation notice naming the file, and `cyber doctor` warns about it

#### Scenario: Many imports
- **WHEN** `/repo/CYBER.md` imports enough files to exceed the total instruction budget
- **THEN** the baseline stops at the budget with a truncation notice, and a warning names each file left out

#### Scenario: Complete instruction assembly is bounded
- **WHEN** instruction files, imports, labels or omitted-file lists exceed the configured total
- **THEN** the complete rendered instruction-file baseline (`core/instructions`) and each nested reminder SHALL fit their respective byte budgets, including labels, notices, separators and wrappers
- **AND** only complete file labels actually delivered SHALL consume reminder identity; unlisted imports SHALL remain discoverable after their importer is labelled
- **AND** a truncated import declaration SHALL NOT load the file named by its partial path

#### Scenario: Rule review and byte identity
- **WHEN** project rule files are discovered
- **THEN** their exact bounded definition bytes SHALL enter the checkout trust inventory, and held or read-denied definitions SHALL NOT enter privileged model context
- **AND** invalid frontmatter/globs SHALL NOT fall back to unscoped guidance
- **WHEN** a definition changes and is reviewed again
- **THEN** its new content/approval identity SHALL remain independently eligible for delivery

#### Scenario: Compatible durable rule bindings
- **WHEN** a successful matching read delivers a scoped rule
- **THEN** its settlement SHALL retain typed project-root, definition-path, digest and approval-generation metadata separately from canonical instruction-file path markers
- **AND** replay of historical settlements without rule bindings SHALL remain compatible
- **AND** a reviewed definition from another checkout SHALL NOT supply rule context for the parent Session

#### Scenario: Context changes while a successful read is settling
- **GIVEN** a read started in one Location and Context Epoch
- **WHEN** that Epoch becomes stale, is replaced or disappears, or the Session changes Location before reminder settlement
- **THEN** the read result SHALL settle without delivering that operation's nested reminder into the new context
- **AND** the suppressed reminder SHALL NOT consume once-per-Epoch delivery
- **AND** nested discovery SHALL resolve current instruction settings after the successful operation instead of retaining pre-effect permission callbacks
