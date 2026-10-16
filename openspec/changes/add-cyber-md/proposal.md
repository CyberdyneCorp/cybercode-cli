## Why

Claude Code has `CLAUDE.md`, Codex has `AGENTS.md`, and Cyber Code reads both, but it has no instruction file of its own. A team that wants guidance only Cyber Code should follow (its tools, Modes, workflows or skills) has to put it in a file every other agent also reads. The loader is also behind its own spec. `crates/cyber-server/src/runtime/context.rs` lists the file names inline and does no `@path` imports, `AGENTS.override.md` handling, canonical-path deduplication, `instructions.max_bytes` truncation, `CYBER_DISABLE_*` handling or nested on-read discovery, and `cyber doctor` does not show instruction files at all.

## What Changes

- **`CYBER.md`** is Cyber Code's own instruction file, in the role `CLAUDE.md` plays for Claude Code. The global `~/.config/cyber/CYBER.md` loads after the global `AGENTS.md`. In each directory from the project root down to the Location, the order is `AGENTS.md`, `CLAUDE.md` (the existing fallback, only where there is no `AGENTS.md`), `CONTEXT.md`, then `CYBER.md`.
- **One ordered list of file names** (`runtime/instructions.rs`) drives the baseline, nested on-read discovery and `cyber doctor`, so the three cannot drift apart.
- **The spec'd loader behavior now applies to every instruction file:** deduplication by canonical path (a `CYBER.md` symlinked to `AGENTS.md` loads once), `@path` imports with cycle detection and a depth of 5, truncation at 32768 bytes with a notice, `AGENTS.override.md` replacing `AGENTS.md`, `CLAUDE.md` and `CYBER.md` of its directory, `CYBER_DISABLE_CLAUDE_MD`, and `CYBER_DISABLE_PROJECT_CONFIG`.
- **Nested on-read discovery:** a successful built-in `read` of a file under a directory whose instruction files are not in the baseline appends them in a `<system-reminder>`, once per file per Context Epoch, recorded durably on the tool settlement.
- **`cyber doctor`** gains an `instructions` check that lists the files the next Session at the Location loads, in load order, and warns about truncated files and unresolved or refused imports.

Mid-session edits use the existing Context Epoch rules unchanged: an edit arrives as one durable mid-conversation system message, and the cached prefix is never rewritten.

## Capabilities

### Modified Capabilities
- `system-context`: MODIFIED "Instruction file discovery" (adds `CYBER.md` and fixes the per-directory order), "Instruction imports" (adds the project boundary), "Nested rule files on read" (`CYBER.md` and canonical deduplication), and "Path-scoped rules and overrides" (`AGENTS.override.md` also replaces `CYBER.md`).
- `compat-import`: MODIFIED "Read-time instruction compatibility". The compat switches never disable `CYBER.md`.
- `cli-commands`: MODIFIED "Doctor and debug commands" (the `instructions` check).
- `configuration`: MODIFIED "Global config directory" (holds `CYBER.md`).

## Impact

- Code: new `crates/cyber-server/src/runtime/instructions.rs`. `context.rs` delegates to it. `drain.rs` and `auto_override.rs` attach nested instructions at settlement. `ToolSettled` gains `instruction_reminders` and `Epoch` gains `reminded_instructions`; both are serde-defaulted, so old events still decode. `crates/cyber-cli/src/commands/doctor.rs` gets the new check.
- Behavior: existing projects gain truncation (files over 32 KiB), imports, override handling and nested reminders, all already in the spec. Project imports that resolve outside the project are refused with a warning.
- Deferred: a personal `CYBER.local.md` variant, and having `/init` generate `CYBER.md` instead of `AGENTS.md`.
