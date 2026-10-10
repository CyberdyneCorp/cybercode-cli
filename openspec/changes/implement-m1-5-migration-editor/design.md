## Context

Cyber permission evaluation uses ordered last-match-wins rules. Claude groups allow, ask and deny, with deny strongest. Conversion must emit groups in that order while retaining order within each group. File writes and source trust are separate from pure conversion.

## Decisions

Use typed serializable permission rules in a core conversion module. Consume parsed source values, not paths or processes. A conversion batch returns no rules if any rule cannot be mapped without changing its meaning. Errors identify a field/index with a bounded static reason and never echo the offending source value.

Map known native tool actions explicitly. Distinct Claude Edit/Write/MultiEdit/NotebookEdit selectors remain refused until their granularity can be represented: mapping an allow to Cyber's shared edit action would widen authority. This granularity is mandatory remaining work, not a reduced import contract. Preserve resource selectors, stripping a leading project-relative `./`. For Claude Bash prefix selectors, convert the final `:*` suffix to ` *`, matching the canonical import example. Reject malformed selectors, control characters, unknown tools and unmapped modes; subsequent source/report adapters must present these refusals under not imported/manual port rather than silently widen permissions.

Test actual engine decisions using the converted JSON array. Pure conversion does not confer trust, approve tools or permit source execution. Later import review must show every proposed change and remain idempotent and safe against intervening edits. Literal credential conversion and source/provenance reports remain mandatory before accepting full import.

ACP, editor buffer authority, per-Session editor MCP and VS Code are separate required delivery tasks, sharing the server runtime rather than duplicating execution.

## Validation

Canonical Claude examples, deny/ask/allow overlaps, ordered arrays, mode aliases, malformed/unmapped fields and safe error output. Follow with source fixtures, dry-run nonmutation, reviewed-write/idempotency/security regressions and native/editor acceptance as their implementations land.
