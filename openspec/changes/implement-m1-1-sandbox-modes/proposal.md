## Why
M1.1 extends local execution to Windows and completes auto-mode permissions. P1 implementation is authorized while the remaining P0 local-model validation continues independently. P0 already implements the allowlist proxy and most mode selection plumbing; those should be reused and verified.

## What Changes
- Add Windows restricted-token AppContainer execution with scoped filesystem ACLs, credential masking, child-process cleanup and fail-closed behavior.
- Verify the existing allowlist proxy on Windows as well as macOS and Linux, including denied direct network access and approved destinations.
- Add auto-mode classification using the evaluator role, durable decisions, consecutive-block tracking and attended/unattended fallbacks.
- Include auto in Shift+Tab's documented cycle and show pending mode changes until the next Turn.
- Add native Windows CI to prove runtime enforcement, rather than treating a successful cross-build as sandbox evidence.

## Capabilities
### Modified Capabilities
- `sandbox`: Windows enforcement and proxy contract verification.
- `permissions-modes`: P1 cycling and safe auto-mode decisions.

## Impact
Platform process launch, permission host/runtime, TUI mode state, durable event registry and Windows CI. Development proceeds under the active P1 implementation goal; incomplete P0 local-baseline evidence remains an independent release gate. Windows sandbox APIs require native tests; no security claim will rely solely on macOS validation.
