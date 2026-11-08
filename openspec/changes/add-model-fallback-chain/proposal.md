## Why

`fallback_models` was accepted by the configuration (and filled in by `cyber import` from Claude Code's `fallbackModel`), but nothing in the runtime read it. An overloaded provider, an exhausted quota or an expired key failed the Turn, which stops unattended runs (`cyber exec`, Goals, loops, workflows) that another configured model could have finished.

## What Changes

- `provider-catalog` "Model fallback chain" is implemented and made precise: which kinds fall back (and which never do); the failed step is kept and the Turn re-sent on the next resolvable model after the failed one; the chain moves on when a fallback fails; the fallback is tried before a usage limit wait; `session.model.fallback.1` carries `from`, `to`, `kind`, `reason`, `started_ms` and `until_ms`; `fallback.sticky_minutes` is bounded to 1–1440 so the re-sent Turn always uses the fallback; any user model switch ends it; the Session view reports `fallback`; the TUI announces it and shows it in the footer; the keys are trust-gated.
- `configuration` "Top-level keys": adds `fallback`.

## Non-goals

- Per-agent and per-workflow-tier `fallback_models` (the requirement keeps them as MAY).
- Org model policy: none exists yet (Cyber Account, P3), so `ModelDeniedByPolicyError` never occurs and the "never select a denied model" rule has nothing to check until it does.
- Falling back after part of an answer streamed.

## Impact

- `crates/cyber-core`: `model_fallback` settings and validation.
- `crates/cyber-server`: the fallback event, fold and selection, the drain's failure path, the user switch, and the Session view.
- `crates/cyber-tools` and `crates/cyber-app`: `ToolHost::fallback` through the full config resolver.
- `crates/cyber-tui`: the notice and the footer.
- The OpenAPI document and the TypeScript SDK are regenerated.
