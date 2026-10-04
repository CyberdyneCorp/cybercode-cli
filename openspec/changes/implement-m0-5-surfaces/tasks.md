## 1. Server API

- [x] 1.1 Router, tagged errors, envelopes, Location routing, pagination.
- [x] 1.2 Session, message, inbox, revert, shell, command, permission and question routes.
- [x] 1.3 Instance SSE stream with heartbeat; durable Session replay with resume; history.
- [x] 1.4 Basic auth, `auth_token` on stream routes, origin and CORS, durable idempotency keys, compression.
- [x] 1.5 OpenAPI 3.1 from types; route coverage and drift tests.
- [x] 1.6 TCP, peer-checked Unix socket, embedded transport.
- [x] 1.7 WebSocket and stdio JSON-RPC transports with subscriptions and application-registered tools.
- [ ] 1.8 PTY routes and mDNS publishing (deferred).

## 2. Server lifecycle

- [x] 2.1 `cyber-app` assembly; ownership lock; port fallback; password; `server.json` registration.
- [x] 2.2 `cyber serve`, `cyber service start|stop|restart|status|password`, `cyber api`.

## 3. Exec

- [x] 3.1 Prompt assembly, backends (service, embedded, ephemeral, attach), Session selection, modes and never-ask rules.
- [x] 3.2 Output formats, stream-json schema v1, exit codes, budgets, timeout, SIGINT.
- [x] 3.3 `--command`, `--file`; live-verified against OpenAI.

## 4. TUI

- [x] 4.1 Launch flags, transports, composer, history, drafts, external editor.
- [x] 4.2 Mentions, slash commands, shell input, steer and queue, take-back, interrupt.
- [x] 4.3 Rendering, permission and question prompts, pickers (Session rename, fork, archive, delete), themes, title, notifications, exit summary.
- [ ] 4.4 Inline mode, mouse, OSC 52, setup wizard, update prompt, split diffs, syntax highlighting, keybinding config, Session export and child-Session trees in the picker (deferred).

## 5. SDK and docs

- [x] 5.1 `@cyber-code/sdk` generated from `sdk/openapi.json`: typed operations, errors and guards, idempotent retries, resumable streams, prompt wait, permission and question handlers with catch-up, attach/spawn/embedded (stdio) modes, application-registered tools; examples and tests.
- [ ] 5.1a SDK socket transport, `system_prompt`/`append_system_prompt`/`tools` prompt options and runtime schema validators (deferred).
- [ ] 5.2 Generated Rust `cyber-sdk` crate (deferred; `cyber-client` serves the CLI and TUI).
- [x] 5.3 Spec delta, README and ROADMAP status, CI jobs.
