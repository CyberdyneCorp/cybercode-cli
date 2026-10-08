## Why

P1 requires lifecycle hooks, plugin hosting and MCP extensibility. The current configuration accepts arbitrary hook definitions without checking events or handler contracts; no hook dispatcher is implemented.

## What Changes

- Validate and resolve hook definitions before trusted configuration reaches execution consumers.
- Preserve hook contributions and provenance across configuration scopes, with handler-specific trust and managed policy controls.
- Implement all P1 lifecycle events, command/HTTP/prompt/MCP handlers, ordered decision merging, cancellation ownership, durable observability and client/CLI surfaces.
- Implement the JSON-RPC plugin host/package and MCP OAuth/deferred tool search.

## Capabilities

### Modified Capabilities
- `hooks`: deliver the P1 hook configuration and lifecycle contracts.
- `plugins-marketplace`: deliver P1 plugin hosting and package contracts.
- `mcp`: deliver P1 authorization and deferred discovery contracts.

## Impact

Configuration, runtime admission, tool dispatch, storage, API/SDK/TUI/CLI, process ownership and native CI. The first increment validates hook configuration only; it does not enable execution or accept M1.3. All canonical P1 requirements remain in scope.
