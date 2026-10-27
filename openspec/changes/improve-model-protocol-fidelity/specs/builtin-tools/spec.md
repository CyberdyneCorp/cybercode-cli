## ADDED Requirements

### Requirement: Freeform apply_patch
(P1) When the model profile's `edit_tool` is `freeform_patch` and the adapter is OpenAI Responses, `apply_patch` SHALL be offered as a custom tool whose input is the raw patch, constrained by a grammar of the `*** Begin Patch` envelope. The grammar SHALL be written for this product. A `custom_tool_call` SHALL run as `apply_patch { patch }`, and its result SHALL be sent back as `custom_tool_call_output`. Every other adapter SHALL receive the JSON `apply_patch` tool.

#### Scenario: Raw patch from an OpenAI model
- **WHEN** `openai/gpt-6-sol` calls the custom `apply_patch` tool with a raw `*** Begin Patch` text
- **THEN** the patch is applied with the same permission checks and the same result lines as the JSON tool

### Requirement: Tolerant patch matching
(P1) `apply_patch` SHALL look for each hunk's context and removed lines with progressively looser comparisons:
1. exact;
2. trailing whitespace ignored;
3. surrounding whitespace ignored;
4. typographic punctuation folded: curly quotes, dashes and non-breaking spaces.

The first level with exactly one match SHALL win. Several matches at that level SHALL fail the patch as an ambiguous hunk. The parser SHALL accept a patch wrapped in a shell heredoc or in surrounding whitespace.

#### Scenario: Smart quotes in context
- **WHEN** a hunk's context line uses `“` and `”` where the file has `"`
- **THEN** the hunk applies at the single matching location

#### Scenario: Ambiguous loose match
- **WHEN** a hunk matches no location exactly and two locations with whitespace ignored
- **THEN** no file is modified, and the error names the ambiguous hunk

### Requirement: apply_patch through the shell
(P1) A `bash` or `powershell` call whose whole script is an `apply_patch` heredoc, optionally preceded by `cd <dir> &&`, SHALL be executed as `apply_patch` relative to that directory. This applies only when no `apply_patch` executable is on `PATH`. The call SHALL be approved as an `edit`, not as a shell command, and SHALL return the `apply_patch` result. Its record SHALL keep the tool name `bash` with `metadata.intercepted: "apply_patch"`.

#### Scenario: Patch sent through bash
- **WHEN** a model calls `bash` with `apply_patch <<'EOF'` followed by a valid patch and `EOF`
- **THEN** the file is changed through `apply_patch`, the `edit` permission is checked, and the result is `M <path>`
