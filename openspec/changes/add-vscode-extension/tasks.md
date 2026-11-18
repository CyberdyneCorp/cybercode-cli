Specification only (2026-11-18). Resolve the open decisions in design.md before starting section 2.

## 1. Server: ranged `file` parts

- [ ] 1.1 Accept the optional `range { start_line, end_line }` on `file` prompt parts. Apply the `read` tool's checks to the whole path first, then admit only the range (clamp past EOF, refuse malformed). Refuse it on the peer listener as for any `file` part.
  - Tests: "Ranged file part", "Malformed range", a `read` deny with a range still denies, and the idempotency digest includes the range.
- [ ] 1.2 Update OpenAPI (`PromptPart` → `file.range`), regenerate the SDK (`python3 scripts/generate_sdk.py`), and make the TUI's `@path#Lx-y` mentions send ranged `file` parts when the server is local.
  - Tests: `generate_sdk.py --check`; the TUI `parts()` tests updated.

## 2. Package scaffold and build

- [ ] 2.1 Create `clients/vscode/`: `package.json` (`cyber-code.cyber`, `engines.vscode ^1.95.0`, `extensionKind: ["workspace"]`, `capabilities.untrustedWorkspaces` limited), `tsconfig`, ESLint, esbuild script (extension CJS plus webview IIFE), and `npm run package` via `@vscode/vsce`. Depend on the SDK via `file:../../sdk/typescript`, and add `undici` for the socket dispatcher.
- [ ] 2.2 Contributions: the `cyber` view container and `cyber.chat` webview view; the commands Insert Selection Mention, Review Session Changes, Open Session in Terminal, New Terminal Session, Accept, Accept always, Reject and Reject with feedback; the Alt+K/Option+K keybinding; the settings `cyber.executablePath` (scope `machine`), `cyber.autoStartServer` (restricted) and `cyber.includeEditorContext`.
  - Test: a manifest check that there is no setting for passwords or tokens and that `executablePath` is machine-scoped.

## 3. Connection and authentication

- [ ] 3.1 Transport chooser: use the socket on macOS and Linux, use TCP only for a loopback literal, and refuse everything else.
  - Unit tests: "Socket preferred", "LAN registration refused", IPv6 loopback, `localhost`, and a missing registration.
- [ ] 3.2 TCP pre-credential checks (pid alive, health `apiVersion`, `version` and `build`) and the post-auth `service.id` check.
  - Unit tests with a fake server: a mismatched build sends no credential; a mismatched `id` disconnects.
- [ ] 3.3 Auto-start through `execFile(executable, ["service","start"])` with a 10 s timeout, gated by trust and setting.
  - Tests: "Auto-start", "Workspace cannot choose the executable".
- [ ] 3.4 Credential lifecycle: read at connect, memory only, drop on disconnect, deactivation and `401`, one re-read, redacted output-channel logging.
  - Unit tests with an injected `fetch`: no credential in any URL and no `Authorization` in logs; "Password rotated".
- [ ] 3.5 Version and compatibility gate for `apiVersion` and missing routes.

## 4. Webview and bridge

- [ ] 4.1 `src/shared/protocol.ts`: the message union, validators, a 1 MiB cap and ID patterns.
  - Unit tests: unknown type, extra field, oversize, bad ID and "Forged approval refused".
- [ ] 4.2 HTML shell with a per-render nonce CSP, `enableCommandUris: false`, `enableForms: false` and restricted `localResourceRoots`.
  - Unit test: snapshot of the generated CSP (no `connect-src`).
- [ ] 4.3 Inert renderer: text nodes, the Markdown element allowlist, no remote images, and links routed through a host confirmation.
  - Tests: "Injected HTML rendered inert"; a `javascript:` link is not clickable.

## 5. Session binding and streaming

- [ ] 5.1 Location resolution (active editor folder, multi-root picker), Session list and create, and the remembered binding in `workspaceState`.
- [ ] 5.2 History load and the event stream with `durable.seq` resume; delta batching (≤ 60/s) and a 500-message window with "Load older".
  - Tests: "Reload resumes", "Shared with the TUI" (prompt from the TUI appears in the panel).
- [ ] 5.3 Prompt, interrupt, and Mode/model selectors with a pending state until `session.mode.switched.1`.
- [ ] 5.4 Presence lease create, renew every 15 s and release on hide, unbind and deactivate.
  - Test: "Panel hidden" (the lease is released; the ask follows non-interactive policy).

## 6. Editor context and mentions

- [ ] 6.1 Context chip (file plus selection), the dirty-buffer notice with Save, and no chip for untitled or non-file documents.
  - Tests: "Selection sent as a ranged file part", "Denied file withheld".
- [ ] 6.2 Alt+K selection mention (relative, single-line, external absolute path).
  - Test: "Selection reference".

## 7. Approvals and diff review

- [ ] 7.1 Pending-request table: fetch on every (re)connect, then follow `permission.asked.1`, `permission.replied.1` and question events; panel cards; a notification when the panel is hidden.
  - Tests: "Requests from a disconnect", "Stale card cannot approve a later request".
- [ ] 7.2 Reply path: exact request binding, per-decision idempotency key, the unconfirmed state, the `always` pattern preview and the `reject` side-effect notice.
  - Test: "Uncertain reply retried" (network drop injected in the extension host).
- [ ] 7.3 Unified-diff parser and exact-context applier, the `cyber-review:` content provider, single-file `vscode.diff` and multi-file `vscode.changes`, editor-title actions, and the raw-diff fallback.
  - Unit tests: multi-hunk, add and delete file, CRLF, no trailing newline, context mismatch fallback.
  - Integration tests: "Accept a proposed edit", "Reject with feedback", "File changed before review", "Answered in another client".
- [ ] 7.4 Questions, including `plan_exit` choices.
  - Test: "Plan approval as a question".
- [ ] 7.5 `Cyber: Review Session Changes` from `GET .../diff` (read-only).

## 8. Terminal handoff and trust

- [ ] 8.1 Open Session in Terminal and New Terminal Session using `shellPath`/`shellArgs` (no shell string).
  - Test: "Continue in the TUI" (the terminal's creation options are asserted).
- [ ] 8.2 Restricted Mode gating for start, create, prompt, reply and terminal.
  - Test: "Restricted Mode".

## 9. CI and evidence

- [ ] 9.1 Integration harness: `@vscode/test-electron` with a temporary profile, and a fixture modelled on `web/e2e/fixture.ts` (temporary `CYBER_HOME`, loopback scripted provider, real `cyber serve --register`, cleanup).
  - Test: "Test fixture isolation"; a final assertion that the password is absent from settings, storage, logs and webview messages.
- [ ] 9.2 `vscode-extension` job in `.github/workflows/ci.yml` (Linux with `xvfb-run`, macOS) reusing the built `cyber` binary; upload the `.vsix` artifact.
  - Scenario: "Pull request builds the extension".
- [ ] 9.3 Manual acceptance on macOS and Linux with a real provider: chat, selection, diff Accept and Reject, approval from a hidden panel, terminal handoff. Record it in `docs/implementation/vscode-extension.md` with screenshots containing fixture data only.

## 10. Documentation

- [ ] 10.1 `docs/vscode-extension.md`: install from `.vsix`, the security model (loopback or socket only, the password never stored, Restricted Mode), settings, and limits.
- [ ] 10.2 Update the README Known gaps and quick links, `docs/features.md`, `docs/architecture.md` (add the extension to the client diagram) and the ROADMAP M3.7 status.
- [ ] 10.3 Archive this change after acceptance, then run `python3 scripts/spec_inventory.py` (`editor-integration`: −2 removed, +14 added, net +12 P3; `server-api` count unchanged).
