## MODIFIED Requirements

### Requirement: Memory file format
(P1) The system SHALL store each memory as a Markdown file with YAML frontmatter `name` (kebab-case slug, unique per directory), `description` (one line), and `type` (`user`, `feedback`, `project`, or `reference`), followed by the fact. `feedback` and `project` memories SHALL include `**Why:**` and `**How to apply:**` lines. Files with invalid frontmatter SHALL be skipped with a warning.

#### Scenario: Invalid frontmatter
- **WHEN** a memory file lacks `type`
- **THEN** it is not loaded and `cyber debug memory` lists it as invalid

### Requirement: Index loading
(P1) The system SHALL load each directory's `MEMORY.md` into the `core/memory` Context Source at baseline initialization, truncated to the first 200 lines or 25,000 bytes, whichever is smaller, with a truncation notice. Individual memory files SHALL be read on demand through the memory tool, not loaded in full.

#### Scenario: Oversized index
- **WHEN** `MEMORY.md` has 340 lines
- **THEN** the first 200 lines are loaded followed by `[memory index truncated; read MEMORY.md for more]`

### Requirement: Secret redaction
(P1) The system SHALL reject a memory write whose content matches secret patterns (API keys, tokens, private keys, passwords, high-entropy strings over 32 characters) with `Memory rejected: content looks like a secret`, and SHALL never write credentials into memory.

#### Scenario: API key in a memory
- **WHEN** the model tries to save `OPENAI_API_KEY=sk-...` as a reference memory
- **THEN** the write fails and nothing is stored


#### Scenario: Quoted credential text refused
- **WHEN** a proposed memory body or frontmatter contains a quoted JSON or YAML credential assignment
- **THEN** shared write validation SHALL refuse it with `Memory rejected: content looks like a secret` without echoing the credential

### Requirement: Memory locations
(P1) The system SHALL keep project memory in `~/.local/share/cyber/memory/<project_id>/` and global memory in `~/.local/share/cyber/memory/global/`, each containing one Markdown file per memory and a `MEMORY.md` index. The `global` project ID SHALL map to the global directory.

#### Scenario: Project memory directory
- **WHEN** a Session runs in a repository with project ID `prj_7f3a`
- **THEN** project memories are read from and written to `~/.local/share/cyber/memory/prj_7f3a/`

### Requirement: Memory toggles
(P1) The system SHALL honor `memory.enabled` (default true; when false, no memory is loaded and the tool is not offered) and `memory.generate` (when false, memory is loaded read-only and `write`, `update`, and `delete` are not offered), exposed through `/memory on|off|readonly` and the `CYBER_DISABLE_MEMORY` environment variable.

#### Scenario: Read-only memory
- **WHEN** the user runs `/memory readonly`
- **THEN** the index is still loaded but the memory tool offers only `list` and `read`

#### Scenario: Invalid memory toggle type
- **WHEN** loaded configuration supplies nonboolean `memory.enabled` or `memory.generate`, or a nonobject `memory` value
- **THEN** configuration loading SHALL fail with a memory validation error without echoing supplied values
