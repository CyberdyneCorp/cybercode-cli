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

## Incremental CLI detection

Dispatch detection before Context bootstrap/logging. Use metadata-only libgit2 worktree discovery, explicit HOME/Location/CODEX_HOME inputs, verified source snapshots and a sixteen MiB aggregate parse budget. Parse TOML using the maintained serde-compatible toml parser and JSONC using the existing core parser, replacing parser details with static issues. Count raw definitions/files without returning values or executing handlers. Incomplete referenced-source/session/compatibility resolution is explicit, with unknown fields and false completeness flags; this increment does not meet the complete canonical Source detection contract.

## Codex provider/model source adapter

Parse at most one MiB TOML into the pure provider adapter; accept at most 256 top-level fields, 64 providers, 64 fields per provider, 32 headers per collection and 16 KiB scalar strings. Convert explicit `chat` to native openai-compatible and `responses`/the documented omitted default to openai-responses. Preserve model-provider qualification and register selected custom models using native defaults with unknown pricing. Convert existing environment credential/header names into references; replace literal API keys/bearer tokens and all static header values with deterministic collision-free environment variables and report which variables must be set without carrying source values. Endpoints containing declared literal credentials/header values use environment references as well. Refuse known credentials reused in retained metadata or existing environment references, conflicting generated environment bindings, ambiguous credentials/headers, account-auth sources, endpoint credentials/query/fragment/placeholders and malformed names. Unknown source/provider fields are indexed pending mappings with values withheld, not implied successful conversion. This adapter never writes, runs helpers, opens keyring storage or claims complete source migration.

## Windows review identity handles

Capability directory handles intentionally deny delete sharing while path lookups occur. Keep them during bounded reads and fresh verification, then reopen the same directory objects as attribute-only, delete-sharing identity handles before handing the snapshot to review. Compare native identities before releasing lookup handles and freshly verify the namespace after the transition. Do not perform any path operation through the retained identity handles. This preserves anti-reuse evidence without freezing user directory renames; failed handoff refuses. Existing parent/junction replacement assertions and a whole-project-root rename regression require native Windows execution.

## Partial read-only configuration previews

Dispatch `cyber import <tool|auto> --dry-run` before Context/bootstrap. Build a held-source proposal using static discovery, bounded source snapshots and native `cyber.json`/`cyber.jsonc` snapshots. Project proposals overlay source global/project layers; global proposals exclude project sources. Overlay raw source families before conversion so inherited Codex provider/protocol selections survive. Preserve existing destination keys; auto fills missing keys in opencode/codex/claude order. Recheck read snapshots and absent native files before output. Do not create directories, logs, databases or configuration.

Output normalized, redacted JSON unified diffs, not exact original-byte/comment-preserving diffs. Redact both raw and JSON-escaped known credential values, including occurrences in unrecognized fields; refuse new secret-bearing strings and executable file substitutions. Existing native arrays overlay rather than concatenate. Supported Claude/Codex permission groups retain strongest-effect ordering. Manual-port/unimplemented source kinds are explicit; completeness remains false. Source reports currently identify a representative file for layered converted families rather than full per-leaf provenance; required environment records omit values. Complete canonical source mapping, effective native ancestor/global layer resolution, exact file diffs, full provenance, reviewed transactions and annotations remain mandatory before acceptance.

Windows review handoff now directly requests only FILE_READ_ATTRIBUTES from ReOpenFile with read/write/delete sharing, compares full identities and then releases capability lookup handles. Static native-error categories permit diagnosis without error text or source content. This is an attempted correction requiring fresh native execution; all existing rename/replacement assertions remain unchanged.

## Native file-layer preservation during preview

The proposal baseline must include explicit native global files and every project file layer in runtime order: root-to-Location `cyber.json`/`cyber.jsonc`, followed by root-to-Location `.cyber/cyber.json`, `.cyber/cyber.jsonc` and `.cyber/cyber.local.jsonc`. Share file-path enumeration and raw merge semantics with the runtime loader, including its array policies. Keep every existing snapshot and every missing candidate under review; refuse changed/replaced/new files. Bound total native plus source parsed bytes to sixteen MiB and native candidate count to 4096. Do not substitute environment/file references, resolve profiles, apply transient flags, trust project configuration or bootstrap resources. Output explicitly describes a raw file-layer proposal, with paths of native layers influencing it; complete runtime-effective profile/env/trust handling and byte-exact destination writes remain required.
