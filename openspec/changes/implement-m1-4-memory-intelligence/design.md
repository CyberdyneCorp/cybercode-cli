## Memory foundation

Pure core types validate required YAML fields and kebab-case names, one-line descriptions, known memory types and nonempty content. Feedback/project notes require nonempty Why and How to apply lines. Parse diagnostics do not echo supplied text. Write validation scans the entire input, including frontmatter, for credential patterns, private keys and high-entropy strings longer than 32 characters. Parsing an existing document remains distinct from admitting a write.

Index rendering sorts unique validated names, uses local Markdown filenames and escapes link labels/descriptions. Duplicate names are refused at this layer; storage will replace matching names before rendering. Index loading keeps the first 200 lines or 25,000 UTF-8 bytes, whichever is smaller, and adds the canonical truncation notice. Typed settings default enabled/generate to true; the existing EnvSource flag semantics implement CYBER_DISABLE_MEMORY. Project IDs are validated as safe components before deriving data/memory paths.

This first increment has no filesystem CRUD or runtime entry point. Following work must retain cross-process mutation ownership, recover file/index interruption consistently, preserve user edits, enforce private file modes and reject path/symlink escapes before effects. Tools, HTTP, CLI/TUI and Context Source reconciliation must use the same validation and storage owner. Read-only generation and disabled memory must be enforced at dispatch, not only in model instructions.

## Complete milestone delivery

Implement the remaining memory contract, then language-server and formatter ownership, diagnostics feedback and isolated browser verification with revision-linked artifacts. Full native acceptance, complete P1 contracts and all milestone exit gates remain required; pure validation tests cannot accept those integrations.
