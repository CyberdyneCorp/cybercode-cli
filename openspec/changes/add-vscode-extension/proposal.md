## Why

Claude Code and Codex both ship a VS Code extension. Users get a chat panel next to their code, the editor's selection and open file as context, proposed edits in the native diff editor with Accept/Reject, and approvals without switching to a terminal. Cyber Code has no IDE client: the README's Known gaps lists "IDE/desktop clients" for every comparison, and `editor-integration` is a target spec with no implementation.

Most of what such a client needs already exists and is covered by tests: one background server per user, the public API, durable Session event replay, interactive presence leases, permission and question requests, and the TypeScript SDK (`@cyber-code/sdk`) that the web dashboard's security model was designed around. A first extension can be a thin client of those contracts. The one thing missing is a credential. Today the only local credential is the owner password, which grants every owner route. A long-lived editor client should hold something narrower that the owner can revoke.

The current `editor-integration` requirements describe the finished product: Marketplace and Open VSX publication, per-hunk review, plan documents, the `ide/context` source with diagnostics, `cyber ide install` and deep links. They do not say how the extension authenticates, what the webview may do, or how it is built, tested and released. This change defines a shippable first slice (MVP) in the current phase (P1), states its security contract precisely, and splits the remaining parts into separate P3 requirements.

## What Changes

- **MVP extension, in-repo at `clients/vscode/`**, built on `@cyber-code/sdk`. The manifest uses the placeholder publisher `cyber-code` (ID `cyber-code.cyber`) until the organization reserves a Marketplace publisher.
  - **Sidebar chat panel** (webview view) bound to one Session in the workspace folder's Location. It streams the Session's durable events with `durable.seq` resume, and supports prompt, interrupt, Session list, new Session and resume, plus Mode and model selectors. The Mode selector offers no `bypass` or `dont-ask`.
  - **Editor context**: an "active file / selection" chip sent as a server-read `file` part with an optional line range. Server-side `read` checks therefore apply, so deny rules (for example `.env`) withhold the content. Alt+K inserts `@path#L<start>-<end>`.
  - **Native diff review of pending edits**: a `permission.asked.1` edit request opens VS Code's diff (or multi-file diff) editor between a read-only base snapshot and the proposed content. Accept, Accept always, Reject and Reject with feedback answer the exact pending request. Completed Session changes can be reviewed read-only.
  - **In-editor approvals**: permission and question cards in the panel, plus a notification when the panel is hidden. Replies are idempotent and bound to the exact request. Stale cards are disabled, and `always` shows the scope it saves. `plan_exit` arrives as an ordinary question.
  - **Server start/attach**: reuse the registered server. Otherwise ask once whether to start the background server. The answer (`always` or `never`) is remembered in the machine-scoped setting `cyber.autoStartServer` (`ask` by default) and can be changed in settings. Starting runs `cyber service start` (argv, no shell) in a trusted workspace only.
  - **The `cyber` CLI is required on `PATH`**, or set through the machine-scoped `cyber.executablePath`. The extension does not bundle a binary.
  - **Terminal handoff**: "Open Session in Terminal" runs `cyber --resume <id>` in an integrated terminal.
- **Local IDE tokens (server, new credential)**:
  - `cyber ide token create --client vscode` issues a `cyi_` token through owner-only routes in the `ide` group (`POST|GET /api/v1/ide/tokens`, `DELETE /api/v1/ide/tokens/:tokenID`). It writes the token to `<state>/ide/vscode.token` (0600, in a 0700 directory) and never prints it.
  - The server stores only the token's SHA-256. Tokens expire after 30 days by default (90 at most). There is one active token per client, so reissuing rotates. Tokens can be listed (`cyber ide token list`) and revoked (`cyber ide token revoke`).
  - A token is accepted only on the Unix socket or from a loopback peer address. It is refused on the peer and collaboration listeners and in `auth_token` query parameters.
  - A token authorizes exactly the routes the extension needs: Sessions, prompt, interrupt, Mode and model, messages, events, diff, presence leases, permission and question requests and replies, models and service identity. Every other route, minting tokens, and switching to `bypass` or `dont-ask` get `403` with `reason: "ide_token_scope"`.
  - Revocation closes the token's streams within 1 s. Replacing the owner password revokes every IDE token.
- **Security contract for the extension**:
  - The extension connects over the Unix socket on macOS and Linux, and over TCP only to a loopback literal.
  - It authenticates only with the IDE token and never reads the owner password. When no token exists, it opens a terminal running `cyber ide token create --client vscode`, so the owner issues the token visibly.
  - The token is held only in extension-host memory. It never goes into settings, SecretStorage, extension state, logs, URLs or the webview.
  - The webview has no credentials and no network access. It has a strict nonce CSP, a schema-checked message bridge and inert rendering.
  - The server's Origin/CORS policy is unchanged.
  - VS Code Restricted Mode is honored.
- **Packaging, CI and release**:
  - The build is an esbuild bundle packaged with `@vscode/vsce`.
  - CI runs unit tests, plus `@vscode/test-electron` integration tests against the real `cyber` binary and a scripted provider, on Linux (xvfb) and macOS. The `.vsix` is a CI artifact.
  - The release workflow attaches the unsigned `cyber-<version>.vsix` and its `.sha256` to each GitHub release.
- **Server**: the `file` prompt part gains an optional `range { start_line, end_line }`, read through the same `read` checks.
- **Phase**: the MVP requirements are (P1), in the current phase. Marketplace and Open VSX publication, per-hunk review, plan documents, ACP, IDE context, IDE detection, deep links (P3) and JetBrains (P4) keep their later phases.
- **Re-scoping of `editor-integration`**:
  - "VS Code extension" now defines the MVP surface and `.vsix` distribution.
  - Marketplace and Open VSX publication moves to "Extension marketplace publication", which uses the reserved publisher ID.
  - "Inline diff review" is split. Per-request review becomes "Pending edit diff review", and per-hunk actions and post-hoc revert become "Per-hunk diff review".
  - "Selection mentions and plan review" is split into "Selection mentions" and "Plan review in the editor".
  - Every existing scenario is kept in one of the new requirements.
  - ACP, "IDE context source", "IDE detection and extension install", "Deep links" and "JetBrains plugin" are unchanged.

### ACP: the next editor deliverable

The VS Code extension is built on the public API through the SDK, not on ACP. `cyber acp` (already specified, unchanged) is the next editor deliverable after this MVP, for Zed, JetBrains, Neovim and third-party ACP clients. VS Code has no built-in ACP client; ACP support there comes from community extensions. An ACP process is also a single stdio session, while the public API gives shared Sessions with the TUI and dashboard, durable replay, presence leases and the full approval model. See design.md D1.

## Non-goals

- Publishing to the VS Code Marketplace or Open VSX, and signing the `.vsix` ("Extension marketplace publication", P3). The organization's publisher ID is reserved before that.
- Bundling a `cyber` binary in the extension, or per-platform `.vsix` files.
- A server proof-of-possession handshake before credentials are sent over TCP. It is deferred to Windows support, the only platform where the extension would use TCP instead of the Unix socket.
- JetBrains, Cursor/Windsurf-specific behavior beyond what plain `.vsix` install gives, and `cyber acp` itself, which is the follow-up.
- Remote use: VS Code Remote-SSH, WSL, Dev Containers, Codespaces, `vscode.dev`, or a server that is not on the extension host's own machine. The extension refuses non-loopback registrations, and the server refuses IDE tokens from non-loopback peers.
- Per-hunk accept/reject, reverting already-applied edits from the diff view, and rendering plan documents.
- `POST /api/v1/ide/context`, diagnostics sharing, `/ide`, `cyber ide install`, IDE detection, the `vscode://` URI handler and `cyber://` deep links.
- IDE tokens for clients other than `vscode`, and tokens bound to particular Locations.
- Windows: no Windows CLI release ships yet. The TCP loopback path is specified but only exercised on Linux and macOS CI.

## Capabilities

### New Capabilities

None. The extension belongs to the existing `editor-integration` capability, and IDE tokens to `server-api`.

### Modified Capabilities

- `editor-integration`:
  - MODIFIED "VS Code extension" (now P1).
  - REMOVED (split, scenarios carried over) "Inline diff review" and "Selection mentions and plan review".
  - ADDED (P1): "Pending edit diff review", "Selection mentions", "Extension server connection", "Extension server auto-start consent", "Extension IDE token authentication", "Extension webview isolation", "Extension workspace trust", "Chat panel Session binding", "Editor context attachment", "In-editor approvals", "Session terminal handoff", "Extension packaging and CI", "Extension release assets".
  - ADDED (P3): "Extension marketplace publication", "Per-hunk diff review", "Plan review in the editor".
- `server-api`:
  - MODIFIED "Local password authentication" names the IDE token as the only other local credential.
  - MODIFIED "Prompt attachment parts" adds the optional (P1) `range` to `file` parts.
  - ADDED (P1) "Local IDE token issuance", "Local IDE token scope", "Local IDE token revocation and expiry".
- `cli-commands`: MODIFIED "Resource command groups" adds `ide` subcommands `token create`, `token list` and `token revoke`.

No requirement in `permissions-modes`, `client-sdk`, `web-client` or `workspace-trust` changes. The extension uses their existing contracts: replies and approvals, interactive presence leases, durable replay, `cyber service start` and `cyber --resume`.

## Impact

- **Code**:
  - new `clients/vscode/` (TypeScript extension, webview UI, tests);
  - `crates/cyber-server`: IDE token storage, issuance, scope enforcement and revocation, the `file` part range, and OpenAPI plus regenerated SDK schema;
  - `crates/cyber-cli`: `cyber ide token create|list|revoke`, and revoking IDE tokens on `cyber service password`.
- **Data model**: a new server database table for IDE token records (ID, client, name, SHA-256, timestamps, revocation), with a migration.
- **CI and release**:
  - a new `vscode-extension` job (build, typecheck, lint, unit tests, `@vscode/test-electron` on Linux and macOS, `.vsix` artifact); the existing `ci.yml` path filter already treats `clients/` as code;
  - `release.yml` builds and attaches the `.vsix` and its SHA-256.
- **Security**: a new, narrower local credential and a new local client. design.md D3–D6 cover the threat model.
- **Docs**: `docs/vscode-extension.md` (new), `docs/web-dashboard.md` (the IDE token next to the owner password), README Known gaps and `docs/features.md`, all updated at implementation time. This change updates ROADMAP.md (P1 entry, M3.7 note, phase overview).
