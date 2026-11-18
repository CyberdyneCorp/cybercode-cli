Specification only (2026-11-18). The owner decided all open questions on 2026-11-18 (design.md, Owner decisions). Implementation starts after this change merges.

## 0. Prerequisites

- [ ] 0.1 Reserve the organization's Visual Studio Marketplace publisher ID and record it in `docs/vscode-extension.md`. Until then the manifest uses the placeholder publisher `cyber-code`. After reservation, replace the placeholder in `clients/vscode/package.json`, the `editor-integration` spec (VS Code extension, Extension marketplace publication, IDE detection and extension install) and `cyber ide install`. This must happen before any Marketplace or Open VSX publication.

## 1. Server: local IDE tokens

- [ ] 1.1 Storage: a migration adding the IDE token table (`id` `itk_…`, `client`, `name`, `sha256`, `created_at`, `expires_at`, `last_used_at`, `revoked_at`), with one active row per client, and deletion of revoked or expired rows after 30 days.
  - Tests: the migration from the previous schema; an old database opens; retention deletes only rows older than 30 days.
- [ ] 1.2 Issuance routes in the `ide` group (`POST|GET /api/v1/ide/tokens`, `DELETE /api/v1/ide/tokens/:tokenID`), owner Basic only. The token is `cyi_` plus 32 random bytes in base64url, returned once and stored as SHA-256. Expiry defaults to 30 days, 1–90 allowed. Issuing for the same client rotates in one transaction. Unknown `client` is refused.
  - Tests: "Issue a token" (server side), "Reissue rotates", "A token cannot mint tokens", "Listing hides secrets"; `expires_in_days: 0` and `91` are refused; idempotent DELETE.
- [ ] 1.3 Authentication middleware: recognize `Bearer cyi_…` before account JWT validation; compare in constant time; accept only on the Unix socket or from a loopback peer on the main listener; refuse on the peer and collaboration listeners, from non-loopback peers and in `auth_token`.
  - Tests: "Token from the LAN refused"; the peer listener returns `401`; an `auth_token` carrying a `cyi_` value returns `401`; a malformed token returns `401` without touching the database beyond one indexed lookup.
- [ ] 1.4 Scope enforcement: one allowlist table of operation IDs checked before handler dispatch, refusing others with `403 ForbiddenError { reason: "ide_token_scope" }`; refuse `POST .../mode` with `bypass` or `dont-ask`.
  - Tests: "Out-of-scope route", "Bypass mode refused", "IDE token is not an owner credential". A table-driven test walks every operation in `sdk/openapi.json` and asserts that exactly the allowlisted operations accept a token. "Approval with an IDE token" (the log names the `itk_` ID and never the value).
- [ ] 1.5 Revocation and expiry: `401` on the next request; open event streams of the token closed within 1 s; presence leases created by it ended; `last_used_at` throttled to once per minute; `cyber service password <value>` revokes all tokens.
  - Tests: "Revocation closes streams", "Expiry" (injected clock), "Password replacement revokes IDE tokens", tokens survive restart.
- [ ] 1.6 CLI `cyber ide token create|list|revoke`: atomic 0600 write to `<state>/ide/<client>.token` in a 0700 directory; print only the ID and expiry; `revoke` deletes the file when it holds the revoked token; `list --format json`.
  - Tests: "Issue a token" (CLI), "IDE token subcommands"; file and directory modes; no token value in stdout, stderr or logs; `<state>/ide/` unreadable from a sandboxed `bash` call (extends the existing private-state test).
- [ ] 1.7 OpenAPI entries for the token routes and the `ForbiddenError` reason; regenerate the SDK.
  - Test: `python3 scripts/generate_sdk.py --check`.

## 2. Server: ranged `file` parts

- [ ] 2.1 Accept the optional `range { start_line, end_line }` on `file` prompt parts. Apply the `read` tool's checks to the whole path first, then admit only the range (clamp past EOF, refuse malformed). Refuse it on the peer listener as for any `file` part.
  - Tests: "Ranged file part", "Malformed range", a `read` deny with a range still denies, and the idempotency digest includes the range.
- [ ] 2.2 Update OpenAPI (`PromptPart` → `file.range`), regenerate the SDK, and make the TUI's `@path#Lx-y` mentions send ranged `file` parts when the server is local.
  - Tests: `generate_sdk.py --check`; the TUI `parts()` tests updated.

## 3. Package scaffold and build

- [ ] 3.1 Create `clients/vscode/`: `package.json` (name `cyber`, placeholder publisher `cyber-code`, `engines.vscode ^1.95.0`, `extensionKind: ["workspace"]`, `capabilities.untrustedWorkspaces` limited), `tsconfig`, ESLint, esbuild script (extension CJS plus webview IIFE), and `npm run package` via `@vscode/vsce`. Depend on the SDK via `file:../../sdk/typescript`, and add `undici` for the socket dispatcher. No bundled `cyber` binary.
- [ ] 3.2 Contributions: the `cyber` view container and `cyber.chat` webview view; the commands Insert Selection Mention, Review Session Changes, Open Session in Terminal, New Terminal Session, Create IDE Token, Accept, Accept always, Reject and Reject with feedback; the Alt+K/Option+K keybinding; the settings `cyber.executablePath` (scope `machine`), `cyber.autoStartServer` (`ask`/`always`/`never`, default `ask`, scope `machine`, restricted) and `cyber.includeEditorContext`.
  - Test: a manifest check that there is no setting for passwords or tokens, and that `executablePath` and `autoStartServer` are machine-scoped.

## 4. Extension connection and authentication

- [ ] 4.1 Transport chooser: use the socket on macOS and Linux, use TCP only for a loopback literal, and refuse everything else.
  - Unit tests: "Socket preferred", "LAN registration refused", IPv6 loopback, `localhost`, and a missing registration.
- [ ] 4.2 TCP pre-credential checks (pid alive, health `apiVersion`, `version` and `build`) and the post-auth `service.id` check. No proof-of-possession handshake (deferred to Windows support).
  - Unit tests with a fake server: a mismatched build sends no credential; a mismatched `id` disconnects.
- [ ] 4.3 Executable resolution (machine setting, then `PATH`) and the missing-CLI message.
  - Tests: "Workspace cannot choose the executable", "Missing executable".
- [ ] 4.4 Auto-start consent: the `ask` modal with `Always start`, `Start this time` and `Don't start`; persisting `always`/`never` to user settings; the `Start server` action under `never`; `execFile(executable, ["service","start"])` with a 10 s timeout; trusted workspaces only.
  - Tests: "Asked once", "Declined", "Changed in settings".
- [ ] 4.5 IDE token authentication: read `<state>/ide/vscode.token` at connect and keep it in memory only; Bearer header; drop it on disconnect, deactivation and `401`, then one re-read; the `Create IDE token` terminal action and file watch; the 7-day expiry notice; the `ide_token_scope` message; never open `<state>/password`; redacted output-channel logging.
  - Unit tests with an injected `fetch`: no credential in any URL, no `Authorization` in logs, `password` never opened.
  - Integration tests: "First run without a token", "Token revoked", "Out-of-scope action".
- [ ] 4.6 Version and compatibility gate for `apiVersion`, missing routes and a `503` `ide` group.

## 5. Webview and bridge

- [ ] 5.1 `src/shared/protocol.ts`: the message union, validators, a 1 MiB cap and ID patterns.
  - Unit tests: unknown type, extra field, oversize, bad ID and "Forged approval refused".
- [ ] 5.2 HTML shell with a per-render nonce CSP, `enableCommandUris: false`, `enableForms: false` and restricted `localResourceRoots`.
  - Unit test: snapshot of the generated CSP (no `connect-src`).
- [ ] 5.3 Inert renderer: text nodes, the Markdown element allowlist, no remote images, and links routed through a host confirmation.
  - Tests: "Injected HTML rendered inert"; a `javascript:` link is not clickable.

## 6. Session binding and streaming

- [ ] 6.1 Location resolution (active editor folder, multi-root picker), Session list and create, and the remembered binding in `workspaceState`.
- [ ] 6.2 History load and the event stream with `durable.seq` resume; delta batching (≤ 60/s) and a 500-message window with "Load older".
  - Tests: "Reload resumes", "Shared with the TUI".
- [ ] 6.3 Prompt, interrupt, and Mode/model selectors (no `bypass` or `dont-ask`) with a pending state until `session.mode.switched.1`.
- [ ] 6.4 Presence lease create, renew every 15 s and release on hide, unbind and deactivate.
  - Test: "Panel hidden".

## 7. Editor context and mentions

- [ ] 7.1 Context chip (file plus selection), the dirty-buffer notice with Save, no chip for untitled or non-file documents, and the whole-file fallback for servers without `range`.
  - Tests: "Selection sent as a ranged file part", "Denied file withheld".
- [ ] 7.2 Alt+K selection mention (relative, single-line, external absolute path).
  - Test: "Selection reference".

## 8. Approvals and diff review

- [ ] 8.1 Pending-request table: fetch on every (re)connect, then follow `permission.asked.1`, `permission.replied.1` and question events; panel cards; a notification when the panel is hidden.
  - Tests: "Requests from a disconnect", "Stale card cannot approve a later request".
- [ ] 8.2 Reply path: exact request binding, per-decision idempotency key, the unconfirmed state, the `always` pattern preview and the `reject` side-effect notice.
  - Test: "Uncertain reply retried".
- [ ] 8.3 Unified-diff parser and exact-context applier, the `cyber-review:` content provider, single-file `vscode.diff` and multi-file `vscode.changes`, editor-title actions, and the raw-diff fallback.
  - Unit tests: multi-hunk, add and delete file, CRLF, no trailing newline, context mismatch fallback.
  - Integration tests: "Accept a proposed edit", "Reject with feedback", "File changed before review", "Answered in another client".
- [ ] 8.4 Questions, including `plan_exit` choices.
  - Test: "Plan approval as a question".
- [ ] 8.5 `Cyber: Review Session Changes` from `GET .../diff` (read-only).

## 9. Terminal handoff and trust

- [ ] 9.1 Open Session in Terminal and New Terminal Session using `shellPath`/`shellArgs` (no shell string).
  - Test: "Continue in the TUI".
- [ ] 9.2 Restricted Mode gating for start, create, prompt, reply, terminal and Create IDE Token.
  - Test: "Restricted Mode".

## 10. CI, release and evidence

- [ ] 10.1 Integration harness: `@vscode/test-electron` with a temporary profile, and a fixture modelled on `web/e2e/fixture.ts` (temporary `CYBER_HOME`, loopback scripted provider, real `cyber serve --register`, an IDE token issued through the real CLI, cleanup).
  - Test: "Test fixture isolation"; a final assertion that the token is absent from settings, storage, logs and webview messages, and that `<state>/password` was never opened.
- [ ] 10.2 `vscode-extension` job in `.github/workflows/ci.yml` (Linux with `xvfb-run`, macOS) reusing the built `cyber` binary; upload the `.vsix` artifact.
  - Scenario: "Pull request builds the extension".
- [ ] 10.3 `release.yml`: build the extension from the tag; check that `clients/vscode/package.json` matches the tag version; generate `cyber-<version>.vsix.sha256`; attach both to the GitHub release and list them in the release manifest; add an "unsigned extension" note with the `sha256sum -c` command to the release notes; fail the release on any packaging or checksum error.
  - Tests: "Release carries the extension" (dry-run release on a test tag), "Version mismatch blocks the release".
- [ ] 10.4 Manual acceptance on macOS and Linux with a real provider: token creation from the panel, chat, selection, diff Accept and Reject, approval from a hidden panel, revocation while attached, terminal handoff. Record it in `docs/implementation/vscode-extension.md` with screenshots containing fixture data only.

## 11. Documentation

- [ ] 11.1 `docs/vscode-extension.md`: install from the `.vsix` and verify the SHA-256, the `cyber` on `PATH` requirement, IDE tokens (create, list, revoke, expiry, rotation), the security model (socket or loopback only, scope, Restricted Mode), settings, and limits.
- [ ] 11.2 `docs/web-dashboard.md` and `docs/local-core-guide.md`: IDE tokens as the second local credential, and that `cyber service password` revokes them.
- [ ] 11.3 Update the README Known gaps and quick links, `docs/features.md`, `docs/architecture.md` (add the extension to the client diagram) and the ROADMAP P1 entry status.
- [ ] 11.4 Archive this change after acceptance, then run `python3 scripts/spec_inventory.py`. Expected counts:
  - `editor-integration`: −2 removed P3, +13 P1, +3 P3, and "VS Code extension" moving from P3 to P1, which gives P1 +14 and P3 +0;
  - `server-api`: +3 P1;
  - `cli-commands`: unchanged.

## 12. Follow-up (not in this change)

- [ ] 12.1 `cyber acp` (the existing ACP requirements of `editor-integration`) is the next editor deliverable after this MVP.
- [ ] 12.2 The TCP proof-of-possession handshake, together with Windows support.
- [ ] 12.3 Marketplace and Open VSX publication with the reserved publisher ID (requires 0.1).
