## Context

Cyber permission evaluation uses ordered last-match-wins rules. Claude groups allow, ask and deny, with deny strongest. Conversion must emit groups in that order while retaining order within each group. File writes and source trust are separate from pure conversion.

## Decisions

Use typed serializable permission rules in a core conversion module. Consume parsed source values, not paths or processes. A conversion batch returns no rules if any rule cannot be mapped without changing its meaning. Errors identify a field/index with a bounded static reason and never echo the offending source value.

Map known native tool actions explicitly. The subsequent scoped-rule implementation preserves distinct Edit/Write/MultiEdit/NotebookEdit selectors with the exact native tool identity rather than broadening an allow to the shared edit action. Preserve resource selectors, stripping a leading project-relative `./`. For Claude Bash prefix selectors, convert the final `:*` suffix to ` *`, matching the canonical import example. Reject malformed selectors, control characters, unknown tools and unmapped modes; subsequent source/report adapters must present these refusals under not imported/manual port rather than silently widen permissions.

Test actual engine decisions using the converted JSON array. Pure conversion does not confer trust, approve tools or permit source execution. Later import review must show every proposed change and remain idempotent and safe against intervening edits. Literal credential conversion and source/provenance reports remain mandatory before accepting full import.

ACP, editor buffer authority, per-Session editor MCP and VS Code are separate required delivery tasks, sharing the server runtime rather than duplicating execution.

## Validation

Canonical Claude examples, deny/ask/allow overlaps, ordered arrays, mode aliases, malformed/unmapped fields and safe error output. Follow with source fixtures, dry-run nonmutation, reviewed-write/idempotency/security regressions and native/editor acceptance as their implementations land.

## Edit selector delivery

Extend native ordered rules with an optional exact tool name; omit it from serialization for legacy unscoped rules. Requests carry an optional tool identity filled by the host from the immutable invocation. Use the same scope match for normal rules, explicit deny ceilings, saved-rule matching and protected-path exact allows. Existing saved approvals remain deliberate native action/resource grants and cannot defeat a scoped explicit deny. Catalog filtering uses each definition's actual name plus its existing permission action. Write and NotebookEdit each emit one scoped rule; Edit/MultiEdit emit an edit-scoped rule at the same original source rule position. No patch grant is inferred because patch operations can also create/remove files. A patch-preferring adapter retains its ordinary patch approval requirement instead of receiving an implicit imported grant. Unknown/malformed scope data is fail-closed. Preserve all existing modes and public action semantics.

## OpenCode explicit permissions

Convert legacy tools first and explicit permission rules afterward. Preserve serde_json's written map order; reject simultaneous singular/plural fields rather than inventing precedence. Map the canonical source aliases to native shared actions. OpenCode edit governs the whole modification group, unlike the distinct Claude selectors. Convert a source pattern ending in ` *` to ` **`: equivalent literal glob matching without Cyber's special bare-prefix match. Refuse unresolved home patterns until source discovery supplies an explicit source home. Unsupported safety guards/actions must not disappear silently. This step does not import implicit source defaults or claim the full OpenCode source requirement.

## Codex argv prefix conversion

Consume parsed constant prefix-rule records, never execute source Starlark. Expand position alternatives with a batch-wide 4096-rule ceiling. Require ASCII literal tokens whose spelling is preserved by native command analysis and glob normalization; reject whitespace, wildcard, quoting, backslash and shell metacharacters rather than widening grants. Append the native space-star prefix suffix, which includes both exact argv prefixes and trailing arguments. Stable-sort by decision strength (allow, ask, deny), because Codex uses strongest-match precedence, not source last-match. The source parser and inline examples/metadata, all config/profile/sandbox adapters and review/write/report delivery remain mandatory subsequent tasks.

Native command permission resources contain source text, so the glob projection alone cannot represent exact argv-prefix decisions under quoting or whitespace. Retain argv_prefix metadata alongside the canonical human-readable resource. Parse a single literal command without expansion/control syntax using the existing Bash tree-sitter parser, then decode quoting with shell-words. Match token prefixes case-sensitively, including on Windows. Unknown/nonliteral command text must never match an imported allow and conservatively retains ask/deny boundaries. Malformed configured argv metadata becomes a deny, not an unscoped allow. Legacy rules remain byte-compatible when the metadata is absent.

## Literal rules source parser

Parse constant prefix_rule calls with a bounded literal reader, not a Starlark evaluator. Support comments, quoted strings/lists and only the documented keyword fields. Preserve rationale and tokenized inline examples in typed source records, verify examples against their own prefix alternatives, and convert the entire batch with the existing precedence/expansion limits. Reject dynamic constructs/unknown fields atomically with an indexed or offset-only error; the future import report must make these unsupported sources visible. This parser does not imply arbitrary Starlark compatibility or complete migration acceptance.

## Read-only file inventory

Use explicit canonical root inputs, walk project layers from repository root to the current Location and retain source/layer/kind/path provenance. Inventory known global/project files and bounded source directories without reading source values or following symlinks. Report linked/uninspectable entries rather than treating them as absent. Retain both default and distinct explicit Codex homes, leaving semantic source precedence to the source adapter. Deterministic tool order is opencode, codex, claude. This inventory is prerequisite delivery, not parsed detection counts/read-time status or the import CLI. Source-referenced custom paths and session/database inspection remain required.

## Verified source snapshots

Admission checks the static inventory and explicit layer root, then opens the complete path from its volume/root through no-follow component handles. Retain native identities and compare fresh path bindings, file identity, modification metadata and a bounded content digest. Windows uses existing full native file identities and reparse refusal without requiring memory ACL policy; Unix compares device/inode and refuses multiply-linked files. Do not expose bytes in Debug or errors. This is the source-review primitive; atomic destination writes and review/confirmation/idempotency remain mandatory.
