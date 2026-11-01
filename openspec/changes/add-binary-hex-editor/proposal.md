## Why

Cyber can edit source through model tools and preview text in `/files`, but a binary currently becomes only `binary · <size>`. Users and agents cannot inspect exact offsets, compare bytes or make a reviewed binary patch within the same terminal workspace. A fast, polished hex workspace would make firmware, fixtures, file headers and other binary work a useful part of daily coding.

The existing explorer is a browser/preview surface, not a general manual file editor. This extension adds explicit byte editing rather than pretending that the text preview already writes files. It supports the project's goal of being an excellent standalone CLI alongside Codex, Claude Code, Pi and OpenCode; no comparative capability or quality claim is made without measurement.

## What Changes

- Add a theme-native hex workspace from the explorer, `/hex <path>` and `cyber hex <path>`: offset gutter, grouped hex bytes, synchronized ASCII, selection, a value inspector, change rail and command hints.
- Add exact-offset navigation, bounded literal-byte/UTF-8 search, named bookmarks and explicit text/hex viewing, including non-UTF-8 and NUL-containing files.
- Stage overwrite/insert/delete operations with local undo/redo and a before/after byte diff. Saving uses an expected-base revision, permission checks, private preimage/recovery evidence and a durable operation receipt.
- Add paged byte/draft/save operations to the existing owner `fs` API and generated SDK, and a bounded model-facing `binary` tool for inspect/read/search/patch. Preserve existing text-tool and attachment contracts.
- Provide byte-oriented snapshot/rewind evidence without relying on text diff, without claiming to reverse remote effects, and without silently dropping large files that ordinary snapshots skip.

## Capabilities

### New Capabilities

- `binary-editing`: exact byte access, draft/patch/save/recovery semantics, bounds, authority and model operation contracts.

### Modified Capabilities

- `tui`: hex workspace, explorer entry, explicit edit/review states and theme/accessibility behavior.
- `cli-commands`: `hex` command registration and direct terminal entry.
- `server-api`: typed byte/draft/save routes in the existing `fs` group, with owner and Session/Location scope.
- `builtin-tools`: capability-owned `binary` tool registration.
- `snapshots-checkpoints`: byte-aware restore receipts and truthful binary diff/rewind coverage.

## Impact and delivery boundary

Primary areas are `cyber-core` file identity/path admission, a reusable byte document/patch service, `cyber-server` owner routes and runtime mutation/receipt ownership, `cyber-tools`, `cyber-client`, the TypeScript SDK, `cyber-tui` and snapshot/recovery storage. Byte state belongs to the server; the TUI does not write arbitrary local paths on its own.

Implementation was approved on 2026-10-27. On 2026-11-01 the user prioritized issue #94 ahead of the unfinished Plugins/hooks and Daily coding workstreams. Shared snapshot/rewind and FileChanged integration must still reuse their contracts; changing the schedule does not accept unimplemented prerequisites or native save guarantees. New native computer ports, a complete text editor, disassembly/decompilation, format repair, live process/device memory, image manipulation and plugin-defined binary parsers are outside this change.
