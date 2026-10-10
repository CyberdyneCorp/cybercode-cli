## Why

P1 requires lifecycle hooks, plugin hosting, MCP and skill/command extensibility. This change tracks their full canonical P1 contracts while retaining native and milestone acceptance gates.

## What Changes

- Validate and resolve hook definitions before trusted configuration reaches execution consumers.
- Preserve hook contributions and provenance across configuration scopes, with handler-specific trust and managed policy controls.
- Implement all P1 lifecycle events, command/HTTP/prompt/MCP handlers, ordered decision merging, cancellation ownership, durable observability and client/CLI surfaces.
- Implement the JSON-RPC plugin host/package and MCP OAuth/deferred tool search.
- Deliver P1 remote skill sources, scoped tool approvals/model selection, shell-output command injection, bundled skills and path-triggered/forked skills.

## Capabilities

### Modified Capabilities
- `hooks`: deliver the P1 hook configuration and lifecycle contracts.
- `plugins-marketplace`: deliver P1 plugin hosting and package contracts.
- `mcp`: deliver P1 authorization and deferred discovery contracts.
- `skills-commands`: deliver the full canonical P1 skill and command contracts.

## Impact

Configuration, runtime admission, tool dispatch, storage, API/SDK/TUI/CLI, process ownership and native CI. The first increment validates hook configuration only; it does not enable execution or accept M1.3. All canonical P1 requirements remain in scope.
