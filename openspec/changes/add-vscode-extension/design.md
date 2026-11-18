## Context

Cyber Code runs one background server per user (`server-api` → One server per user). The TUI, `cyber exec`, the embedded web dashboard and SDK scripts are all clients of the owner API. Several existing contracts shape this design:

- **Discovery.** `<state>/server.json` (0600) records `id`, `version`, `build`, `binary`, `url`, `socket` and `pid`. `cyber service start` reuses a healthy registered server or spawns `cyber serve --register` (`server-api` → Background service management).
- **Listener.** TCP binds `127.0.0.1:4747` by default (or an OS-assigned port). On macOS and Linux the server also listens on `<state>/cyber.sock` (0600) (`server-api` → Listener defaults).
- **Authentication.** Every owner route except health needs HTTP Basic `cyber:<password>`, read from `<state>/password` (0600). The `auth_token` query parameter exists only for browsers that cannot set headers on streams (`server-api` → Local password authentication).
- **Origin.** A request whose `Origin` is absent, matches the Host, is `http://localhost:*`/`http://127.0.0.1:*`, is `tauri://localhost` or appears in `remote.cors_origins` is allowed. Any other Origin gets `403` (`server-api` → Origin and CORS policy).
- **Presence.** Interactive presence leases are created, renewed every 15 s and expire after 45 s. They let the server ask a human but never grant permissions (`server-api` → Interactive clients). The dashboard is the reference client.
- **Requests.** Permission requests carry `metadata.diff` (a unified diff with `a/`/`b/` headers and 3 lines of context), `resources` and `always_patterns`. A reply is `once`, `always` or `reject` with an optional message. Any `reject` declines every other pending request of the Session (`permissions-modes` → Replies and approvals). Edit tools write only if the file is unchanged since it was read for approval (guarded write).
- **SDK.** `@cyber-code/sdk` (ESM, Node ≥ 20, no runtime dependencies) reads the registration and password, sends Basic headers or a Bearer token (`auth: { type: "account", token }` already sends `Authorization: Bearer`), resumes Session event streams by `durable.seq`, retries non-GET calls with stable `Idempotency-Key`s, and accepts an injected `fetch`.
- **Dashboard security model** (docs/web-dashboard.md). Credentials live in memory only. Content is rendered inert. Stale cards cannot approve later requests. A reply is shown only after the server acknowledges it.

Claude Code's extension uses a different shape. The extension hosts a loopback WebSocket MCP server, and the CLI connects to it with a per-activation token from a 0600 lock file. Codex's extension drives its own local agent process. Both give a chat panel, editor context, native diff review and in-editor approvals. Cyber already has a long-lived, multi-client server, so the extension is simply one more client of it.

## Goals / Non-Goals

**Goals:**
- A user who opens VS Code in a repository can chat with a Session, attach the selection, review a proposed edit in the native diff editor and approve or reject it without opening a terminal. The same Session stays live in the TUI and dashboard.
- The extension holds less authority than the TUI: a scoped, revocable IDE token instead of the owner password. It adds no listener or CORS origin.
- Every behavior is testable in CI against the real binary.

**Non-Goals:** see proposal.md.

## Decisions

### D1. Public API through the SDK; ACP is the next editor deliverable

| | Public API + SDK (chosen) | ACP over stdio (`cyber acp`) |
|---|---|---|
| VS Code support | Ours; full control of UI | No built-in VS Code client (community extensions only) |
| Session sharing with TUI/dashboard | Same live Session, concurrent clients | One process per editor connection |
| Durable replay after reload | `durable.seq` resume | `session/load` replays everything |
| Approvals | Full model: `always` scope, taint, child routing, presence | `allow_once`/`allow_always`/`reject` |
| Workflows, Jobs, Goals, budgets | Available | Not in protocol |
| Reuse for other editors | No | Zed, JetBrains, Neovim, Emacs |

ACP is the right protocol for editors we do not write a client for, so it stays in `editor-integration` unchanged. It should be built right after this MVP as a thin adapter over the same server. *Rejected:* building the extension on ACP. That would put a lossy protocol between our own UI and our own server and lose shared Sessions.

### D2. Where the code lives and how it is built
- `clients/vscode/` is a standalone npm package (`private: true`, `engines.vscode: ^1.95.0`, `engines.node: >=20`). It depends on the SDK through `file:../../sdk/typescript`; the SDK is not on npm yet.
- **Layout.** `src/extension/` holds extension-host code (activation, connection, Session controller, diff provider, approvals, terminal). `src/webview/` holds the panel UI (plain TypeScript and DOM, no framework). `src/shared/` holds the message schema used by both sides.
- **Build.** esbuild produces `dist/extension.js` (CommonJS, platform node, `vscode` external) and `dist/webview.js` (IIFE, browser). `@vscode/vsce package --no-dependencies` produces `cyber-<version>.vsix`. Nothing loads at runtime from a CDN.
- *Rejected:* putting the extension in `web/`. The dashboard bundle is embedded in the Rust binary and has a reproducibility check, and the extension has a different release path.

### D3. Connection and transport security
1. **Discovery.**
   - Read `server.json` through the SDK's `readRegistration`.
   - With no healthy registration, in a trusted workspace only, consult the machine-scoped `cyber.autoStartServer` (`ask` by default, or `always` or `never`). With `ask`, show a modal once: `Always start` and `Don't start` write `always` and `never` to user settings, and `Start this time` writes nothing.
   - Machine scope keeps the answer per machine, out of workspace settings and out of Settings Sync. The user changes it in settings.
   - Starting runs `<executable> service start` with `execFile` (argv, no shell, 10 s timeout), then reads the registration again.
   - `<executable>` is `cyber.executablePath` (scope `machine`, so `.vscode/settings.json` cannot set it), otherwise `cyber` resolved on `PATH`. No binary is bundled. If none is found, the panel links to the install instructions.
2. **Transport choice.**
   - On macOS and Linux, use the registered `socket` when present. The SDK gets an injected `fetch` bound to an undici `Agent({ connect: { socketPath } })`.
   - Otherwise use TCP only when the registered `url` is `http://127.0.0.1:<port>`, `http://[::1]:<port>` or `http://localhost:<port>`. Any other host, including a TLS LAN hostname, is refused with `Cyber Code server is not on loopback; the VS Code extension supports local servers only`.
   - Why the socket first: it lives in the private state directory, so another local user cannot squat it. A loopback TCP port can be taken by another local user after a stale registration is left behind.
3. **TCP pre-credential checks.** Before sending credentials over TCP, the extension SHALL:
   - check that the registration `pid` is alive;
   - check that unauthenticated `GET /api/v1/health` returns `apiVersion: "v1"` and the registration's `version` and `build`;
   - after the first authenticated call, check that `GET /api/v1/service` returns the registration `id`.
   This narrows the stale-port window but does not close it, because the token is sent with the first authenticated call.
   - The token is scoped and revocable, and the server refuses it from non-loopback peers, so a squatter that captures it gets only the IDE scope, until the owner revokes or rotates the token.
   - A server proof-of-possession handshake would close the window. It is deferred to Windows support, the only platform where the extension would use TCP.
4. **Version skew.** If `apiVersion` is not `v1`, or a route the MVP needs answers 404 or 503, the panel shows the server version and stops. It degrades no further.

### D4. Credential: local IDE tokens
**Why a new credential.** The owner password authorizes every owner route: credentials, providers, configuration, PTYs, workflows, service stop, password replacement. An editor client is long-lived, sits next to many other extensions, and needs only about 20 routes. A scoped token limits what a leaked credential can do and can be revoked without rotating the owner password and restarting every client.

**Issuance (owner only).**
- `cyber ide token create --client vscode [--name] [--expires <days>]` calls `POST /api/v1/ide/tokens` with owner Basic credentials. The default is 30 days and the maximum 90.
- The server generates 32 random bytes, returns `cyi_<base64url>` once, and stores only its SHA-256 plus the record `{ id: itk_…, client, name, created_at, expires_at, last_used_at, revoked_at }` in a new table.
- The CLI writes the token atomically to `<state>/ide/vscode.token` (0600, in a 0700 directory) and prints only the ID and expiry. It never prints the token, to keep it out of scrollback and terminal logs.
- There is one active token per client, so issuing another rotates: the old token is revoked in the same transaction.
- **How the extension gets one.** The extension never holds owner credentials, so it cannot mint a token. When the file is missing, or the token is rejected twice, or it expires within 7 days, the panel offers `Create IDE token`. That opens an integrated terminal running `cyber ide token create --client vscode`, so the owner sees and runs the command, and the extension watches for the file.
- The state directory is already private to the owner: model tools and sandboxed children cannot read it ("Private owner authentication storage"), so they cannot read the token either.

**Presentation and transport.**
- The token is sent as `Authorization: Bearer cyi_…`. The `cyi_` prefix lets the server tell it apart from Cyber Account JWTs (`eyJ…`) before any JWT validation.
- It is accepted only on the Unix socket or from a loopback peer address on the main listener. It is refused on the peer listener, the collaboration listener, non-loopback peers (a TLS LAN bind) and the `auth_token` query parameter.

**Scope (an allowlist of routes, enforced before the handler runs).**

| Purpose | Routes |
|---|---|
| Identity and models | `GET /api/v1/service`, `GET /api/v1/models` |
| Sessions | `GET\|POST /api/v1/sessions`, `GET .../:sessionID`, `POST .../prompt`, `.../interrupt`, `.../mode`, `.../model`, `GET .../messages`, `.../events`, `.../diff` |
| Presence | `POST .../interactive-clients`, `PUT\|DELETE .../interactive-clients/:clientID` |
| Approvals | `GET /api/v1/permissions/requests`, `POST .../permissions/:requestID/reply`, `GET /api/v1/questions/requests`, `POST .../questions/:requestID/reply` |

- Everything else is refused with `403 ForbiddenError { reason: "ide_token_scope" }`. That includes the token routes themselves, so a token cannot mint, list or revoke tokens.
- `POST .../mode` with `bypass` or `dont-ask` is also refused, so a leaked token cannot remove human approval.
- Permission rules, Modes, sandbox, trust and `file` part checks apply unchanged. A token request is never more privileged than the owner.

**Revocation, expiry and rotation.**
- A revoked or expired token gets `401` on its next request. Revocation closes the token's event streams within 1 s and ends the presence leases it created.
- `cyber service password <value>` revokes every IDE token, because replacing the owner password is the "credentials may be compromised" action.
- Tokens survive server restarts. Records stay listable for 30 days after revocation or expiry.
- **Audit.** The server logs the `itk_` ID, never the value, and updates `last_used_at` at most once a minute to bound writes.

**In the extension.**
- The token is read when a connection is created and kept in the connection's closure. It is dropped on disconnect, deactivation and `401`, after which the file is re-read once.
- It is never stored in VS Code settings, `SecretStorage`, `globalState`/`workspaceState`, the output channel, telemetry, error messages, URLs (Node `fetch` sets headers on SSE) or any webview message. There is no setting that accepts a credential.
- The extension never opens `<state>/password`, and an integration test asserts that.
- **Logs.** The output channel logs method, route template, status and request ID only.

*Rejected alternatives:*
- **Owner password.** It gives full authority, and revoking it means rotating the owner password.
- **SecretStorage copy.** It adds a second place to rotate and leak, with no isolation gain inside the same-user extension host.
- **Extension-initiated pairing with a code shown in the TUI.** It needs a new interactive flow. The terminal command gives the same owner-visible consent with existing pieces.
- **Location-bound tokens.** VS Code workspaces often span folders. Binding tokens to Locations is a possible follow-up.

### D5. Webview isolation (CSRF and origin)
- **The webview never talks to the server.** Its origin is `vscode-webview://…`, which the server's Origin policy already rejects with `403`. This change does not add it to `remote.cors_origins`, and the extension SHALL NOT ask users to add it. All API traffic leaves the extension host, whose Node `fetch` sends no `Origin`. A malicious web page therefore cannot ride the extension's credential, because the credential never exists in any browser context.
- **CSP.** `default-src 'none'; script-src 'nonce-<random per render>'; style-src ${webview.cspSource} 'nonce-…'; img-src ${webview.cspSource} data:; font-src ${webview.cspSource}`. There is no `connect-src`, so the webview cannot fetch or open sockets. Options are `enableScripts: true`, `localResourceRoots: [<extension>/dist, <extension>/media]`, `enableCommandUris: false` and `enableForms: false`.
- **Bridge.** Messages are a discriminated union defined in `src/shared/protocol.ts` and validated on both sides. Unknown types, extra fields, oversized payloads (>1 MiB) and IDs that do not match `^(ses|per|que)_[A-Za-z0-9]+$` are dropped and logged. From the webview, approval messages carry `{ sessionId, requestId, reply, message? }` only. The extension host looks the request up in its own pending table and refuses anything that is not currently pending for the bound Session.
- **Inert rendering.** Model, tool and file text is rendered as text nodes. Markdown is converted to a fixed allowlist of elements (p, code, pre, em, strong, ul, ol, li, a, h1–h4, blockquote, table parts). Links render as plain text with their URL visible; opening one goes through the host and `vscode.env.openExternal` after a confirmation. Remote images are never loaded. This matches the dashboard's untrusted-content rules.

### D6. Workspace trust
- `capabilities.untrustedWorkspaces: { supported: "limited", restrictedConfigurations: ["cyber.autoStartServer"] }`.
- In Restricted Mode the extension does not start the server, create Sessions, send prompts or answer approvals. The panel explains why and links to "Manage Workspace Trust".
- VS Code trust does not replace Cyber's own checkout trust (`workspace-trust`), which the server still enforces. The two are independent gates.

### D7. Session binding and streaming
- **Location.** The Location is the workspace folder that contains the active editor, falling back to the first folder. It is sent as `x-cyber-directory` via `client.at(dir)`. Multi-root workspaces show a folder picker in the panel header.
- **Binding.** The panel binds to one Session at a time, chosen from `session.list` for that Location (newest first) or newly created. The bound Session ID is remembered per workspace in `workspaceState`. This is an ID only, not content or a credential.
- **Events.** The extension subscribes with `client.sessions.events(id, { after })` and keeps the last `durable.seq` in memory. Reopening the panel refetches messages via `message.list` and resumes the stream. Live text deltas update the open assistant message, and durable events are authoritative.
- **Presence.** While the panel is visible and bound, the extension creates an interactive presence lease and renews it every 15 s. It releases the lease on hide, unbind or deactivate. With no lease, pending asks follow unattended policy, exactly as for the dashboard.
- **Controls.** Interrupt uses `POST .../interrupt`. Mode and model use `POST .../mode` and `.../model`, shown as pending until `session.mode.switched.1` arrives.

### D8. Editor context
- **The chip.** The composer shows the active editor file and, when one exists, the selection range. It is on by default and the user can remove it (`cyber.includeEditorContext`, default `true`).
- **Wire format.** On send the extension adds `file { path, range? }`, where `path` is relative to the Location and `range` is 1-based inclusive. The server reads it through the `read` tool's checks, so deny rules, hidden paths, hooks, `external_directory` and taint apply. The extension never inlines file content itself. *Rejected:* inlining text like the TUI does. That would bypass `read` deny rules for files such as `.env`. Claude Code's extension withholds such files too.
- **Unsaved buffers.** When the document is dirty, the chip shows "unsaved changes not included" and offers Save. Unsaved buffer reads are part of the ACP write-through requirement, not this MVP.
- **Mention insertion.** Alt+K (Option+K) inserts `@<path>#L<a>-<b>` into the composer and focuses the panel. The server resolves it the same way.

### D9. Diff review of pending edits
- **Trigger.** A `permission.asked.1` with `action: "edit"` and `metadata.diff` (from `edit`, `write`, `apply_patch` or formatters) creates a review.
- **Building the review.** The extension parses the unified diff per file. It reads the current on-disk content with `workspace.fs` and stores it as the **base snapshot**. It applies the hunks with exact context matching to produce the **proposed** content. Both are served read-only by a `TextDocumentContentProvider` under `cyber-review:` URIs that embed the request ID and an opaque per-file token, never the file content.
- **Opening it.** One file opens `vscode.diff(base, proposed, "<path> (Cyber proposal)")`. Several files open the multi-file diff editor (`vscode.changes`).
- **Actions.** Editor-title actions (context key `cyber.reviewPending`) and the panel card offer **Accept** (`once`), **Accept always** (only when `always_patterns` is non-empty; shows the patterns first), **Reject** and **Reject with feedback** (an input box, sent as `message`).
- **Fallback.** If the hunks do not apply, because the file changed on disk or the buffer is ahead of disk, the review shows the raw diff in a read-only document and says the file changed since the proposal. The server's guarded write would refuse the write anyway.
- **Closing.** After the server acknowledges the reply, or when `permission.replied.1` shows another client answered, the review closes with a status message. Proposed documents are discarded.
- **Completed changes.** "Review Session Changes" opens `GET .../diff` read-only in the multi-file diff editor. Reverting from the diff view is in "Per-hunk diff review" (follow-up).
- *Rejected:* writing the proposal into the real file and letting the user edit before accepting. It races the server's guarded write and breaks snapshot accounting.

### D10. Terminal handoff
"Cyber: Open Session in Terminal" creates an integrated terminal with `shellPath: <executable>`, `shellArgs: ["--resume", <sessionId>]` and `cwd: <Location>`. No shell string is involved. The TUI and the panel then share the live Session.

### D11. Testing strategy
- **Unit (node:test).** Diff parsing and application, protocol validation, CSP generation, the registration and transport chooser (loopback and socket rules), and the credential lifecycle (an injected fake `fetch` asserts no credential in URLs and no logging).
- **Integration (`@vscode/test-electron`, Mocha).** The tests launch VS Code with a temporary profile against a temporary repository and a real `cyber serve --register` with the scripted OpenAI-compatible provider, reusing `web/e2e/fixture.ts` patterns: temporary `CYBER_HOME`, loopback model, no real credentials. Covered scenarios:
  - attach and stream a reply;
  - a selection chip produces a ranged `file` part;
  - an `.env` deny withholds the content;
  - a pending edit opens a diff; Accept writes the file and Reject leaves it unchanged;
  - another client answers first and the review closes;
  - Restricted Mode blocks prompts;
  - a non-loopback registration is refused;
  - IDE token issuance, scope refusal (`403 ide_token_scope`), revocation closing the stream, and `bypass` refused;
  - assertions that the token does not appear in settings, state, logs or webview messages, and that `<state>/password` is never opened.
- CI runs Linux (xvfb-run) and macOS. Windows waits for a Windows CLI release.

## Risks / Trade-offs

- **[Token theft by another same-user extension]** Any extension can read `<state>/ide/vscode.token`, as it could read the owner password. → The token's scope, loopback-only acceptance, 30-day expiry, one-command revocation and rotation limit the damage. The owner password is never exposed to the extension.
- **[Stale loopback port squatting on TCP]** Another local user could squat the port and receive the IDE token. → Prefer the Unix socket on macOS and Linux and run the pid, health and `service.id` checks. A captured token is IDE-scoped and loopback-only. The proof-of-possession handshake is deferred to Windows support.
- **[New credential surface on the server]** A mistake in route classification could widen the scope. → The scope is one allowlist table checked in middleware before routing to handlers. A test enumerates every OpenAPI operation and asserts that only the listed ones accept a token.
- **[Diff application drift]** The proposed view could differ from what the server writes. → Exact-context application with fallback to the raw diff. The server's guarded write is the authority, and the view is advisory.
- **[SDK is ESM-only and unpublished]** → Bundle it with esbuild from the workspace path. Publishing the SDK is a separate decision.
- **[Event volume]** Long Sessions can stream many deltas to the webview. → Batch deltas into one webview message per animation frame (≤ 60/s) and cap the rendered transcript at 500 messages with "Load older".

## Migration Plan

This change only adds things. The IDE token table needs a forward migration. Older servers answer `503` for the `ide` group, so the extension reports the server as incompatible. The server change (an optional `range` on `file` parts) is backward compatible, because older clients never send it. The extension sends `range` only when the server's health `version` is at least the release that ships it. Against an older server it attaches the whole file and labels the chip "whole file". Rollback is uninstalling the `.vsix`.

## Owner decisions (2026-11-18)

1. **Credential**: a scoped, revocable local IDE token is part of the MVP (D4, `server-api` → Local IDE token issuance, scope, revocation and expiry). The extension never uses the owner password.
2. **TCP proof-of-possession handshake**: deferred to Windows support. Recorded as a non-goal.
3. **Auto-start**: ask once, remember the answer per machine in the machine-scoped `cyber.autoStartServer` (`ask`/`always`/`never`), changeable in settings ("Extension server auto-start consent").
4. **Distribution**: the release workflow attaches the unsigned `.vsix` and its SHA-256 to GitHub releases ("Extension release assets").
5. **Publisher ID**: the placeholder `cyber-code` until the organization reserves a Marketplace publisher ID. Reserving it is a task, and a prerequisite for Marketplace publication.
6. **CLI**: `cyber` is required on `PATH`, or set through the machine-scoped `cyber.executablePath`. No bundling.
7. **Phase**: the MVP is P1 (current phase). Follow-ups stay P3.
8. **ACP**: `cyber acp` is the next editor deliverable after this MVP.
