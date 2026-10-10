## Why

M1.4 requires durable memory, code intelligence and isolated browser verification. The current core has project identity and context sources, but no memory document/storage/tool lifecycle. Memory needs a shared validated format before tools, HTTP clients and editor commands can write it consistently.

## What Changes

- Implement all P1 memory contracts: project/global storage, Markdown documents and index, secret rejection, toggles, tools, CLI/TUI, context reconciliation and authenticated HTTP operations/events.
- Implement P1 code intelligence, diagnostics feedback and formatters.
- Implement P1 isolated browser verification and revision-linked artifacts.
- Begin with pure memory document validation, deterministic index construction, bounded index loading and typed configuration. This foundation grants no file or execution authority and does not accept a canonical requirement or M1.4.

## Capabilities

### Modified Capabilities
- `memory`: implement the P1 memory lifecycle and public surfaces.
- `code-intelligence`: implement P1 language intelligence and formatter delivery.
- `builtin-tools`: deliver notebook cell editing needed by the four-tool diagnostics integration.
- `browser-verification`: implement P1 browser execution and linked verification artifacts.

## Impact

Core configuration and document validation, durable filesystem ownership, tools/runtime/context, API/SDK/CLI/TUI and browser/LSP native processes. P0 local-model evidence and backups remain independent and preserved. All P1 requirements remain in the active goal.
