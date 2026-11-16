## Why

A command hook blocks a tool call by exiting with code 2; its stderr becomes the block reason. Hooks often print what they inspected, so that stderr can hold a token or key. The tool result the model receives was already redacted at settlement, but the same reason also went, unredacted and unbounded, into:

- the live `HookNotice` the TUI and every `/api/v1/event` client show;
- the durable hook receipt (`hook.executed.1`), served by `GET /api/v1/sessions/{id}/hook-executions`, `cyber hooks history` and TUI `/hooks history`.

A non-blocking failure (exit 1) put its stderr into a notice the same way, and a JSON `reason` or `stop_reason` took the same paths. The reason was also unbounded: up to the 1 MiB stderr capture reached the model as the tool error.

## What Changes

- `hooks` new requirement "Hook output redaction": a hook's block/deny reason, stop reason and diagnostics are redacted with the Location's tool output redactor (`tool-registry` → Tool output redaction) and bounded to 4 KiB before they become a tool result, a notice, a merged decision or a receipt. Non-secret text stays readable; an input rewrite is applied as the hook returned it.

## Non-goals

- `additional_context` and the rewrite notice keep their current handling.
- Synthetic receipts from `cyber hooks test` are not changed; nothing from them reaches a model.
- Opt-in raw IO (`telemetry.log_hook_io`) stays raw by design.

## Impact

- `crates/cyber-server`: hook text redaction and bound (`runtime/redaction.rs`), applied when a receipt settles.
- `crates/cyber-tools`: each synchronous handler's report is redacted once after it runs; hook notices are redacted for the event's Location.
- Docs: hooks section of `docs/local-core-guide.md`.
