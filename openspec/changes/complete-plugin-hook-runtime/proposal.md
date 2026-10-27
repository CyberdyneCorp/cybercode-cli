## Why

Plugin manifests parse `fs.read` and `shell`, but the current host launch grants Location reads independently of `fs.read` and uses a general sandbox that permits process execution. Runtime-contributed hooks, providers and catalog transforms are reported as unsupported; an authorized `session/prompt` callback also returns unsupported. Several declared hook events lack complete production producers. These gaps prevent completing the existing P1 plugin and hook contracts.

## What Changes

- Enforce the reviewed filesystem and subprocess capability boundary in the native plugin process, including descendants and direct runtime APIs; reject a configuration that cannot enforce it.
- Complete negotiated `hook/<event>`, `provider/stream`, `catalog/transform` and host-to-core `session/prompt`, preserving scope, trust, cancellation, budgets and registration ownership.
- Add missing production sources for `DirectoryAdded`, `FileChanged` and `CwdChanged`; complete the existing `Setup` lifecycle and audit isolated-child `WorktreeCreate` coverage while preserving its pre-effect veto semantics.
- Extend the TypeScript plugin kit and production-path tests; reconcile existing extensibility tasks using evidence rather than unchecked parent headings.

## Capabilities

### New Capabilities

None. This completes existing capabilities.

### Modified Capabilities

- `plugins-marketplace`: enforceable process capabilities, scoped contribution lifecycle and authenticated callback ownership.
- `hooks`: committed event-source semantics and bounded dispatch without mutation replay.

## Impact

Primary areas: `cyber-core` plugin/config types, `cyber-sandbox` platform policies/helpers, `cyber-tools/src/plugin_host`, hook dispatch and runtime/Location/worktree lifecycle, `cyber-llm` adapter integration, plugin SDK and status APIs. Stronger confinement can reject plugins that previously worked by using undeclared access; diagnostics must name the plugin and missing capability or enforcement support.

This is the first workstream in the user-requested goal. Implementation requires review of this proposal, design and tasks. Daily coding follows its accepted implementation. Marketplace distribution, remote plugin sources, mods UI, hosted credentials, new native-provider products and a general Windows sandbox redesign are outside this change.
