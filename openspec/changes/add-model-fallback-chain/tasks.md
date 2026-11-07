## 1. Chain

- [x] 1.1 `fallback_models` and `fallback.sticky_minutes` settings with validation; unit tests.
- [x] 1.2 `session.model.fallback.1`, its fold, and the effective selection the boundary and the automatic switch resolve; a user switch (including to the configured model) and the automatic switch away end it.
- [x] 1.3 The drain's failure path: eligible kinds, the next resolvable model after the failed one, the failed step and the fallback in one commit, before any usage limit wait.
- [x] 1.4 `ToolHost::fallback` through the full resolver, forwarded by the App host.
- [x] 1.5 Runtime tests through the real tool host: overload after retries, stickiness, the chain moving on, a last failure standing, no fallback for other kinds or without a chain, expiry and a user switch.

## 2. Surfaces

- [x] 2.1 The Session view's `fallback`; OpenAPI and SDK regenerated.
- [x] 2.2 The TUI notice and `fallback: <model>` footer, translated; screens tests.
- [x] 2.3 Guide section and feature reference.

## 3. Later

- [ ] 3.1 Per-agent and per-workflow-tier `fallback_models`.
- [ ] 3.2 Skip models denied by org model policy once it exists.
