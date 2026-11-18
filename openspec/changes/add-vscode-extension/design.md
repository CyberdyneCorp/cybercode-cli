## Context

Cyber Code runs one background server per user (`server-api` → One server per user). The TUI, `cyber exec`, the embedded web dashboard and SDK scripts are all clients of the owner API. Several existing contracts shape this design:

- **Discovery.** `<state>/server.json` (0600) records `id`, `version`, `build`, `binary`, `url`, `socket` and `pid`. `cyber service start` reuses a healthy registered server or spawns `cyber serve --register` (`server-api` → Background service management).
- **Listener.** TCP binds `127.0.0.1:4747` by default (or an OS-assigned port). On macOS and Linux the server also listens on `<state>/cyber.sock` (0600) (`server-api` → Listener defaults).
- **Authentication.** Every owner route except health needs HTTP Basic `cyber:<password>`, read from `<state>/password` (0600). The `auth_token` query parameter exists only for browsers that cannot set headers on streams (`server-api` → Local password authentication).
- **Origin.** A request whose `Origin` is absent, matches the Host, is `http://localhost:*`/`http://127.0.0.1:*`, is `tauri://localhost` or appears in `remote.cors_origins` is allowed. Any other Origin gets `403` (`server-api` → Origin and CORS policy).
- **Presence.** Interactive presence leases are created, renewed every 15 s and expire after 45 s. They let the server ask a human but never grant permissions (`server-api` → Interactive clients). The dashboard is the reference client.
- **Requests.** Permission requests carry `metadata.diff` (a unified diff with `a/`/`b/` headers and 3 lines of context), `resources` and `always_patterns`. A reply is `once`, `always` or `reject` with an optional message. Any `reject` declines every other pending request of the Session (`permissions-modes` → Replies and approvals). Edit tools write only if the file is unchanged since it was read for approval (guarded write).
- **SDK.** `@cyber-code/sdk` (ESM, Node ≥ 20, no runtime dependencies) reads the registration and password, sends Basic headers, resumes Session event streams by `durable.seq`, retries non-GET calls with stable `Idempotency-Key`s, and accepts an injected `fetch`.
- **Dashboard security model** (docs/web-dashboard.md). Credentials live in memory only. Content is rendered inert. Stale cards cannot approve later requests. A reply is shown only after the server acknowledges it.

Claude Code's extension uses a different shape. The extension hosts a loopback WebSocket MCP server, and the CLI connects to it with a per-activation token from a 0600 lock file. Codex's extension drives its own local agent process. Both give a chat panel, editor context, native diff review and in-editor approvals. Cyber already has a long-lived, multi-client server, so the extension is simply one more client of it.

## Goals / Non-Goals

**Goals:**
- A user who opens VS Code in a repository can chat with a Session, attach the selection, review a proposed edit in the native diff editor and approve or reject it without opening a terminal. The same Session stays live in the TUI and dashboard.
- The extension introduces no new credential, persistent secret, listener or CORS origin, and it widens no authority beyond what the TUI already has on the same machine.
- Every behavior is testable in CI against the real binary.

**Non-Goals:** see proposal.md.

## Decisions

### D1. Owner API through the SDK; ACP stays a separate complement

| | Owner API + SDK (chosen) | ACP over stdio (`cyber acp`) |
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
   - Read `server.json` through the SDK's `readRegistration`. With no healthy registration, and only in a trusted workspace with `cyber.autoStartServer` (default `true`), run `<executable> service start` with `execFile` (argv, no shell, 10 s timeout) and read the registration again.
   - `<executable>` is `cyber.executablePath` (setting scope `machine`, so `.vscode/settings.json` cannot set it), otherwise `cyber` resolved on `PATH`.
2. **Transport choice.**
   - On macOS and Linux, use the registered `socket` when present. The SDK gets an injected `fetch` bound to an undici `Agent({ connect: { socketPath } })`.
   - Otherwise use TCP only when the registered `url` is `http://127.0.0.1:<port>`, `http://[::1]:<port>` or `http://localhost:<port>`. Any other host, including a TLS LAN hostname, is refused with `Cyber Code server is not on loopback; the VS Code extension supports local servers only`.
   - Why the socket first: it lives in the private state directory, so another local user cannot squat it. A loopback TCP port can be taken by another local user after a stale registration is left behind.
3. **TCP pre-credential checks.** Before sending credentials over TCP, the extension SHALL:
   - check that the registration `pid` is alive;
   - check that unauthenticated `GET /api/v1/health` returns `apiVersion: "v1"` and the registration's `version` and `build`;
   - after the first authenticated call, check that `GET /api/v1/service` returns the registration `id`.
   This narrows the stale-port window but does not close it, because the password is sent with the first authenticated call. A server proof-of-possession handshake would close it; that is open decision 2.
4. **Version skew.** If `apiVersion` is not `v1`, or a route the MVP needs answers 404 or 503, the panel shows the server version and stops. It degrades no further.

### D4. Credential handling
- The password is read with the SDK's `readPassword` from `<state>/password` when a connection is created. It is kept in a closure inside the connection object and is dropped on disconnect, on extension deactivation and on a `401`. A `401` triggers exactly one re-read; another `401` shows "Run `cyber service password` or restart the server".
- **Never stored** in VS Code settings, `SecretStorage`, `globalState`/`workspaceState`, the output channel, telemetry, error messages, URLs (no `auth_token`: Node `fetch` sets headers on SSE) or any message to the webview. There is no setting that accepts a password or token.
- **Logs.** The output channel logs method, route template, status and request ID only. Headers and bodies are never logged.
- *Why not SecretStorage:* the source of truth is already a same-user 0600 file. A copy in the OS keychain would add a second place to rotate and leak without adding any isolation from the extension host, which runs as the same user.
- *Owner authority:* the password grants full owner authority. Any VS Code extension host already runs arbitrary code as that user and could read the same file, so the extension adds no new principal. A scoped IDE credential is still better defense in depth (open decision 1).

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
  - assertions that the password does not appear in settings, state or log output.
- CI runs Linux (xvfb-run) and macOS. Windows waits for a Windows CLI release.

## Risks / Trade-offs

- **[Owner authority in the extension host]** Any other extension can read the same 0600 password file, so this adds no exposure, but the extension is now a valuable target. → Mitigations: no persistence, no webview exposure, strict bridge. Scoped tokens are open decision 1.
- **[Stale loopback port squatting on TCP]** Another local user could squat the port and receive the Basic credential. → Prefer the Unix socket on macOS and Linux, run the pid and health checks, and leave proof-of-possession as open decision 2. The exposure exists only on TCP-only hosts, which today means Windows from source.
- **[Diff application drift]** The proposed view could differ from what the server writes. → Exact-context application with fallback to the raw diff. The server's guarded write is the authority, and the view is advisory.
- **[SDK is ESM-only and unpublished]** → Bundle it with esbuild from the workspace path. Publishing the SDK is a separate decision.
- **[Event volume]** Long Sessions can stream many deltas to the webview. → Batch deltas into one webview message per animation frame (≤ 60/s) and cap the rendered transcript at 500 messages with "Load older".

## Migration Plan

This change only adds things. The server change (an optional `range` on `file` parts) is backward compatible, because older clients never send it. The extension sends `range` only when the server's health `version` is at least the release that ships it. Against an older server it attaches the whole file and labels the chip "whole file". Rollback is uninstalling the `.vsix`.

## Open Questions

1. **Credential.** Keep the owner password for the MVP (as specified), or first add a scoped local "IDE client" token (`cyber ide token`, revocable, limited to session/message/permission/question/fs-read routes)?
2. **Server proof-of-possession on TCP.** Add an unauthenticated `POST /api/v1/ide/handshake` where the server proves it knows the password (an HMAC of a client nonce) before the extension sends the credential, or accept the residual stale-port risk because the socket is the default on macOS and Linux?
3. **Auto-start default.** Should `cyber.autoStartServer` default to `true` (as the TUI does), or ask once per machine before starting a background service that outlives VS Code?
4. **Distribution.** Attach the `.vsix` to GitHub releases now (unsigned, with a SHA-256 in the release manifest), or keep it as a CI artifact until Marketplace and Open VSX publication with signing?
5. **Publisher ID.** Keep `cyber-code.cyber` from the existing spec, or reserve a Marketplace publisher under the organization's name first?
6. **Bundled CLI.** Require `cyber` on PATH (as specified), or bundle a platform-specific binary in per-platform `.vsix` files the way Claude Code bundles its CLI?
7. **Phase.** Keep the MVP at P3 (M3.7) or pull it forward ahead of other P3 connectivity work?
8. **ACP sequencing.** Confirm `cyber acp` is the next editor deliverable after this MVP.
